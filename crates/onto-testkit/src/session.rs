use std::path::PathBuf;

use anyhow::{Context, Result};
use onto_mcp::OntologyServer;
use onto_workspace::{Workspace, proposals};
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

use crate::fixture::Fixture;
use crate::transcript::{Event, Transcript};

/// A live MCP client connected in-process to an [`OntologyServer`] over a
/// duplex pipe: the same protocol path a real host uses, without a subprocess.
pub struct Session {
    client: RunningService<RoleClient, ()>,
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
        Ok(Self { client, root: fixture.root.clone(), transcript: Transcript::default() })
    }

    pub async fn tools(&self) -> Result<Vec<Tool>> {
        Ok(self.client.list_all_tools().await?)
    }

    pub fn instructions(&self) -> Option<String> {
        self.client.peer_info().and_then(|info| info.instructions.clone())
    }

    /// Call a tool and record it. Tool-level errors are returned as `(false, message)`.
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<(bool, Value)> {
        let args = match &arguments {
            Value::Object(map) => map.clone(),
            Value::Null => Default::default(),
            other => anyhow::bail!("tool arguments must be an object, got {other}"),
        };
        let outcome = self.client.call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args)).await;
        let (ok, result) = match outcome {
            Ok(result) => {
                let ok = !result.is_error.unwrap_or(false);
                let value = result.structured_content.clone().unwrap_or_else(|| {
                    let text: Vec<String> = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
                    Value::String(text.join("\n"))
                });
                (ok, value)
            }
            // Protocol errors (unknown tool, invalid arguments) are recorded as failed calls.
            Err(e) => (false, Value::String(e.to_string())),
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
        self.client.cancel().await?;
        Ok(transcript)
    }
}
