use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use onto_mcp::OntologyServer;
use onto_workspace::{Workspace, proposals};
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

use crate::fixture::Fixture;
use crate::transcript::{Event, Transcript};

/// Which tools the agent may see, by name prefix (e.g. `ontology_`, `ods_`).
pub type Visibility = Option<Vec<String>>;

/// MCP clients for an episode: the ontology server in-process over a duplex
/// pipe (the same protocol path a real host uses), optionally OpenDataSuite's
/// `ods mcp` as a child process, with every call recorded in one transcript.
pub struct Session {
    clients: Vec<RunningService<RoleClient, ()>>,
    /// Tool name to the index of the client serving it.
    routes: BTreeMap<String, usize>,
    /// Declared output schemas, checked against every result as strict hosts do.
    output_schemas: BTreeMap<String, Value>,
    visible: Visibility,
    root: PathBuf,
    pub transcript: Transcript,
}

impl Session {
    pub async fn start(fixture: &Fixture) -> Result<Self> {
        let workspace = fixture.workspace()?;
        let store = workspace.open_store()?;
        let server = OntologyServer::new(workspace, store);
        let (server_io, client_io) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let client = ().serve(client_io).await.context("connecting MCP client")?;
        let mut session = Self {
            clients: Vec::new(),
            routes: BTreeMap::new(),
            output_schemas: BTreeMap::new(),
            visible: None,
            root: fixture.root.clone(),
            transcript: Transcript::default(),
        };
        session.add(client).await?;
        Ok(session)
    }

    async fn add(&mut self, client: RunningService<RoleClient, ()>) -> Result<()> {
        let index = self.clients.len();
        for tool in client.list_all_tools().await? {
            if self.routes.contains_key(tool.name.as_ref()) {
                continue;
            }
            self.routes.insert(tool.name.to_string(), index);
            if let Some(schema) = &tool.output_schema {
                self.output_schemas.insert(tool.name.to_string(), Value::Object(schema.as_ref().clone()));
            }
        }
        self.clients.push(client);
        Ok(())
    }

    /// Connect OpenDataSuite's MCP server for the dbt project at `project_dir`.
    pub async fn attach_ods(&mut self, ods: &str, project_dir: &Path, target_dir: &Path) -> Result<()> {
        let mut command = tokio::process::Command::new(ods);
        command.arg("mcp").arg("--project-dir").arg(project_dir).arg("--target-dir").arg(target_dir);
        let transport = TokioChildProcess::new(command).context("starting `ods mcp`")?;
        let client = ().serve(transport).await.context("connecting to `ods mcp`")?;
        self.add(client).await
    }

    /// Only expose tools whose names start with one of `prefixes`.
    pub fn restrict(&mut self, prefixes: &[&str]) {
        self.visible = Some(prefixes.iter().map(|p| p.to_string()).collect());
    }

    fn is_visible(&self, tool: &str) -> bool {
        self.visible.as_ref().is_none_or(|prefixes| prefixes.iter().any(|p| tool.starts_with(p.as_str())))
    }

    pub async fn tools(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        for (index, client) in self.clients.iter().enumerate() {
            for tool in client.list_all_tools().await? {
                if self.routes.get(tool.name.as_ref()) == Some(&index) && self.is_visible(&tool.name) {
                    tools.push(tool);
                }
            }
        }
        Ok(tools)
    }

    /// Server instructions of every connected server with a visible tool.
    pub fn instructions(&self) -> Option<String> {
        let texts: Vec<String> = self
            .clients
            .iter()
            .enumerate()
            .filter(|(index, _)| self.routes.iter().any(|(tool, i)| i == index && self.is_visible(tool)))
            .filter_map(|(_, client)| client.peer_info().and_then(|info| info.instructions.clone()))
            .collect();
        (!texts.is_empty()).then(|| texts.join("\n\n"))
    }

    /// Call a tool and record it. Tool-level errors are returned as `(false, message)`.
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<(bool, Value)> {
        let args = match &arguments {
            Value::Object(map) => map.clone(),
            Value::Null => Default::default(),
            other => anyhow::bail!("tool arguments must be an object, got {other}"),
        };
        let (ok, result) = match self.routes.get(tool).filter(|_| self.is_visible(tool)) {
            None => (false, Value::String(format!("tool `{tool}` is not available"))),
            Some(&index) => match self.clients[index].call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args)).await {
                Ok(result) => {
                    let ok = !result.is_error.unwrap_or(false);
                    let value = result.structured_content.clone().unwrap_or_else(|| {
                        let text: Vec<String> = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
                        Value::String(text.join("\n"))
                    });
                    // Claude Code rejects a result that breaks the tool's declared schema.
                    match (ok, &result.structured_content, self.output_schemas.get(tool)) {
                        (true, Some(content), Some(schema)) => match schema_mismatch(schema, content) {
                            Some(e) => (false, Value::String(format!("Structured content does not match the tool's output schema: {e}"))),
                            None => (ok, value),
                        },
                        _ => (ok, value),
                    }
                }
                // Protocol errors (unknown tool, invalid arguments) are recorded as failed calls.
                Err(e) => (false, Value::String(e.to_string())),
            },
        };
        self.transcript.events.push(Event::ToolCall { tool: tool.to_string(), arguments, ok, result: result.clone() });
        Ok((ok, result))
    }

    pub fn human_says(&mut self, text: impl Into<String>) {
        self.transcript.events.push(Event::Human { text: text.into() });
    }

    pub fn agent_says(&mut self, text: impl Into<String>) {
        self.transcript.events.push(Event::Agent { text: text.into() });
    }

    /// The person accepts a proposal through the CLI path, outside MCP.
    pub fn human_accepts(&mut self, digest: &str, by: &str) -> Result<String> {
        let workspace = Workspace::load(&self.root)?;
        let mut store = workspace.open_store()?;
        let path = proposals::accept(&workspace, &mut store, digest, by)?;
        self.transcript.events.push(Event::HumanAction { action: "accept".into(), detail: json!({ "digest": digest, "by": by }) });
        Ok(path)
    }

    pub async fn close(self) -> Result<Transcript> {
        let transcript = self.transcript;
        for client in self.clients {
            client.cancel().await?;
        }
        Ok(transcript)
    }
}

/// Why `content` does not satisfy `schema`, if it does not.
fn schema_mismatch(schema: &Value, content: &Value) -> Option<String> {
    match jsonschema::validator_for(schema) {
        Ok(validator) => validator.iter_errors(content).next().map(|e| format!("{e} at `{}`", e.instance_path())),
        Err(e) => Some(format!("invalid output schema: {e}")),
    }
}
