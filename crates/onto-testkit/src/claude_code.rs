//! Claude Code as the agent (feature `llm`).
//!
//! Instead of our own model loop, the real Claude Code CLI plays the agent in
//! headless mode (`claude -p`), connected to the same MCP servers a person's
//! host would use: `onto mcp` for the fixture workspace and, when the
//! condition includes it, OpenDataSuite's `ods mcp`. It authenticates however
//! the CLI is set up; in CI that is `CLAUDE_CODE_OAUTH_TOKEN` from
//! `claude setup-token`, so runs use the person's Claude subscription.
//!
//! The scripted person replies by resuming the same Claude Code session.
//! Claude Code's `stream-json` output is converted into our [`Transcript`],
//! so scoring, invariants and the report are the same as for any agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::agent::{EpisodeStats, Reply, TokenUsage, chat, last_proposal};
use crate::fixture::Fixture;
use crate::human::Person;
use crate::transcript::{Event, Transcript};

#[derive(Clone, Debug)]
pub struct ClaudeCodeConfig {
    /// The `claude` executable.
    pub bin: String,
    /// Model name or alias; `None` uses Claude Code's default.
    pub model: Option<String>,
    /// Agentic turns per invocation (`--max-turns`).
    pub max_turns: usize,
    /// The `onto` executable that serves the fixture over MCP.
    pub onto_bin: PathBuf,
    /// Longest one `claude -p` invocation may take.
    pub timeout: Duration,
}

impl ClaudeCodeConfig {
    /// `ONTO_CLAUDE_BIN` (default `claude`), `ONTO_CLAUDE_MODEL`, `ONTO_BIN`
    /// (default `target/debug/onto` in this repository).
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            bin: var("ONTO_CLAUDE_BIN").unwrap_or_else(|| "claude".into()),
            model: var("ONTO_CLAUDE_MODEL"),
            max_turns: 12,
            onto_bin: var("ONTO_BIN").map(PathBuf::from).unwrap_or_else(|| crate::fixture::repo_root().join("target/debug/onto")),
            timeout: Duration::from_secs(600),
        }
    }
}

/// An MCP server Claude Code should connect to.
#[derive(Clone, Debug)]
pub struct Server {
    /// Name as Claude Code sees it; tools appear as `mcp__<name>__<tool>`.
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

impl Server {
    pub fn onto(config: &ClaudeCodeConfig, fixture: &Fixture) -> Self {
        Server {
            name: "onto".into(),
            command: config.onto_bin.to_string_lossy().into_owned(),
            args: vec!["--root".into(), fixture.root.to_string_lossy().into_owned(), "mcp".into()],
        }
    }

    pub fn ods(ods_bin: &str, fixture: &Fixture) -> Self {
        Server {
            name: "ods".into(),
            command: ods_bin.into(),
            args: vec![
                "mcp".into(),
                "--project-dir".into(),
                fixture.root.to_string_lossy().into_owned(),
                "--target-dir".into(),
                fixture.root.join("target").to_string_lossy().into_owned(),
            ],
        }
    }
}

/// `mcp__onto__ontology_entity` → `ontology_entity`.
pub fn tool_name(qualified: &str) -> &str {
    qualified.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")).map(|(_, tool)| tool).unwrap_or(qualified)
}

/// Text of a `tool_result` content field: a string or a list of text blocks.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        other => other.to_string(),
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { format!("{}… ({} chars)", text.chars().take(max).collect::<String>(), text.chars().count()) }
}

/// What one `claude -p` invocation produced.
#[derive(Debug, Default)]
pub struct Invocation {
    pub session_id: Option<String>,
    pub turns: usize,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
    /// Set when Claude Code reported an error result (e.g. max turns).
    pub error: Option<String>,
    /// Tokens this invocation used, from its result event.
    pub usage: TokenUsage,
}

/// Convert Claude Code `stream-json` lines into transcript events, logging
/// each one live. `servers` are the MCP servers that must have connected.
pub fn read_stream(lines: impl IntoIterator<Item = String>, servers: &[String], transcript: &mut Transcript) -> Result<Invocation> {
    let mut out = Invocation::default();
    let mut pending: HashMap<String, (String, Value)> = HashMap::new();
    for line in lines {
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        match event["type"].as_str().unwrap_or_default() {
            "system" if event["subtype"] == "init" => {
                out.session_id = event["session_id"].as_str().map(str::to_string);
                for name in servers {
                    let status =
                        event["mcp_servers"].as_array().and_then(|all| all.iter().find(|s| s["name"] == name.as_str())).and_then(|s| s["status"].as_str());
                    if status != Some("connected") {
                        bail!("MCP server `{name}` did not connect (status: {})", status.unwrap_or("missing"));
                    }
                }
                chat(format!(
                    "· Claude Code session started ({}), servers connected: {}",
                    event["model"].as_str().unwrap_or("default model"),
                    servers.join(", ")
                ));
            }
            "assistant" => {
                for block in event["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or_default().trim().to_string();
                            if !text.is_empty() {
                                chat(format!("ASSISTANT: {text}"));
                                transcript.events.push(Event::Agent { text });
                            }
                        }
                        Some("tool_use") => {
                            let name = tool_name(block["name"].as_str().unwrap_or_default()).to_string();
                            chat(format!("ASSISTANT → {name} {}", block["input"]));
                            pending.insert(block["id"].as_str().unwrap_or_default().to_string(), (name, block["input"].clone()));
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in event["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let (tool, arguments) =
                        pending.remove(block["tool_use_id"].as_str().unwrap_or_default()).unwrap_or_else(|| ("unknown".into(), Value::Null));
                    let ok = !block["is_error"].as_bool().unwrap_or(false);
                    let text = result_text(&block["content"]);
                    chat(format!("TOOL {tool} {}: {}", if ok { "ok" } else { "FAILED" }, clip(&text, 600)));
                    out.tool_calls += 1;
                    if !ok {
                        out.failed_tool_calls += 1;
                    }
                    let result = serde_json::from_str(&text).unwrap_or(Value::String(text));
                    transcript.events.push(Event::ToolCall { tool, arguments, ok, result });
                }
            }
            "result" => {
                out.turns = event["num_turns"].as_u64().unwrap_or(0) as usize;
                out.usage = TokenUsage::from_anthropic(&event["usage"], out.turns.max(1) as u64);
                if event["is_error"].as_bool().unwrap_or(false) || event["subtype"].as_str().is_some_and(|s| s != "success") {
                    let reason = event["subtype"].as_str().unwrap_or("error").to_string();
                    let detail = event["result"].as_str().unwrap_or_default();
                    out.error = Some(if detail.is_empty() { reason } else { format!("{reason}: {}", clip(detail, 300)) });
                }
            }
            "rate_limit_event" if event["rate_limit_info"]["status"] == "rejected" => {
                out.error = Some(format!("Claude usage limit reached: {}", event["rate_limit_info"]));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// How a `claude -p` call gets its instructions.
#[derive(Clone, Copy)]
enum System<'a> {
    /// Claude Code's own system prompt plus ours: the agent, as a real host runs it.
    Append(&'a str),
    /// Ours alone: helper calls (judge, simulated person) that need no agent
    /// prompt, which would otherwise be sent, and paid for, on every call.
    Replace(&'a str),
}

/// Run `claude -p` once in `cwd`, returning its stream-json lines.
async fn invoke(
    config: &ClaudeCodeConfig,
    cwd: &Path,
    mcp_config: &Path,
    servers: &[String],
    system: System<'_>,
    message: &str,
    resume: Option<&str>,
) -> Result<Vec<String>> {
    let mut command = tokio::process::Command::new(&config.bin);
    command
        .arg("-p")
        .arg(message)
        .args(["--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--tools", ""])
        .arg("--mcp-config")
        .arg(mcp_config)
        .arg("--allowedTools")
        .arg(servers.iter().map(|s| format!("mcp__{s}")).collect::<Vec<_>>().join(","))
        .arg("--max-turns")
        .arg(config.max_turns.to_string())
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match system {
        System::Append(text) => command.arg("--append-system-prompt").arg(text),
        System::Replace(text) => command.arg("--system-prompt").arg(text),
    };
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    if let Some(session) = resume {
        command.arg("--resume").arg(session);
    }
    let mut child = command.spawn().with_context(|| format!("starting `{}` (install Claude Code: npm install -g @anthropic-ai/claude-code)", config.bin))?;
    let stdout = child.stdout.take().context("claude stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut collected = Vec::new();
    let read = async {
        while let Some(line) = lines.next_line().await? {
            collected.push(line);
        }
        anyhow::Ok(())
    };
    match tokio::time::timeout(config.timeout, read).await {
        Ok(result) => result?,
        Err(_) => bail!("`claude -p` did not finish within {}s", config.timeout.as_secs()),
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() && collected.iter().all(|l| !l.contains("\"type\":\"result\"")) {
        bail!("`claude -p` exited with {}: {}", output.status, clip(String::from_utf8_lossy(&output.stderr).trim(), 500));
    }
    Ok(collected)
}

/// Run one conversation with Claude Code as the agent against `servers`.
pub async fn run_episode(
    fixture: &Fixture,
    servers: &[Server],
    config: &ClaudeCodeConfig,
    system_prompt: &str,
    opening: &str,
    human: &mut impl Person,
    transcript: &mut Transcript,
) -> Result<EpisodeStats> {
    let scratch = tempfile::tempdir()?;
    let mcp_config = scratch.path().join("mcp.json");
    let mcp: serde_json::Map<String, Value> = servers.iter().map(|s| (s.name.clone(), json!({ "command": s.command, "args": s.args }))).collect();
    std::fs::write(&mcp_config, serde_json::to_vec_pretty(&json!({ "mcpServers": mcp }))?)?;
    // An empty working directory: no project settings, and nothing to read besides the MCP tools.
    let cwd = scratch.path().join("cwd");
    std::fs::create_dir_all(&cwd)?;
    let names: Vec<String> = servers.iter().map(|s| s.name.clone()).collect();

    let mut stats = EpisodeStats::default();
    let mut session: Option<String> = None;
    let mut message = opening.to_string();
    transcript.events.push(Event::Human { text: message.clone() });
    chat(format!("USER: {message}"));
    loop {
        chat(format!("· claude -p{}", if session.is_some() { " --resume" } else { "" }));
        let lines = invoke(config, &cwd, &mcp_config, &names, System::Append(system_prompt), &message, session.as_deref()).await?;
        let invocation = read_stream(lines, if session.is_none() { &names } else { &[] }, transcript)?;
        session = invocation.session_id.or(session);
        stats.turns += invocation.turns;
        stats.tool_calls += invocation.tool_calls;
        stats.failed_tool_calls += invocation.failed_tool_calls;
        stats.usage += invocation.usage;
        if let Some(error) = invocation.error {
            bail!("Claude Code: {error}");
        }
        let Some(session_id) = session.clone() else { bail!("Claude Code reported no session id") };
        let reply = match human.reply(transcript).await? {
            Reply::Say(text) => text,
            Reply::AcceptLastProposal { by, then } => {
                let accepted = match last_proposal(transcript) {
                    Some(digest) => {
                        let workspace = onto_workspace::Workspace::load(&fixture.root)?;
                        let mut store = workspace.open_store()?;
                        onto_workspace::proposals::accept(&workspace, &mut store, &digest, &by)?;
                        transcript.events.push(Event::HumanAction { action: "accept".into(), detail: json!({ "digest": digest, "by": by }) });
                        chat(format!("USER ACTION: accepted proposal {digest}"));
                        format!("I accepted proposal {digest} with the onto CLI.")
                    }
                    None => "I don't see a proposal to accept.".to_string(),
                };
                then.map(|t| format!("{accepted} {t}")).unwrap_or(accepted)
            }
            Reply::End => {
                chat("· the person ends the conversation".to_string());
                break;
            }
        };
        chat(format!("USER: {reply}"));
        transcript.events.push(Event::Human { text: reply.clone() });
        message = reply;
        session = Some(session_id);
    }
    Ok(stats)
}

/// Ask Claude Code one question without tools (for the judge and the simulated
/// person); returns its text answer and the tokens it used.
pub async fn ask(config: &ClaudeCodeConfig, system: &str, prompt: &str) -> Result<(String, TokenUsage)> {
    let scratch = tempfile::tempdir()?;
    let mcp_config = scratch.path().join("mcp.json");
    std::fs::write(&mcp_config, br#"{"mcpServers":{}}"#)?;
    let one_turn = ClaudeCodeConfig { max_turns: 1, ..config.clone() };
    let lines = invoke(&one_turn, scratch.path(), &mcp_config, &[], System::Replace(system), prompt, None).await?;
    let mut answer = String::new();
    let mut usage = TokenUsage::default();
    for line in lines {
        if let Ok(event) = serde_json::from_str::<Value>(&line)
            && event["type"] == "result"
        {
            answer = event["result"].as_str().unwrap_or_default().to_string();
            usage = TokenUsage::from_anthropic(&event["usage"], 1);
        }
    }
    Ok((answer, usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_lose_the_server_prefix() {
        assert_eq!(tool_name("mcp__onto__ontology_entity"), "ontology_entity");
        assert_eq!(tool_name("mcp__ods__ods_lineage"), "ods_lineage");
        assert_eq!(tool_name("Read"), "Read");
    }

    #[test]
    fn stream_json_becomes_a_transcript() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-x","mcp_servers":[{"name":"onto","status":"connected"}]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me look."},{"type":"tool_use","id":"t1","name":"mcp__onto__ontology_entity","input":{"id":"concept:customer"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"{\"found\":true}"}],"is_error":false}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"A customer is an account holder."}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":2,"result":"A customer is an account holder."}"#,
        ];
        let mut transcript = Transcript::default();
        let out = read_stream(lines.iter().map(|l| l.to_string()), &["onto".to_string()], &mut transcript).unwrap();
        assert_eq!(out.session_id.as_deref(), Some("s1"));
        assert_eq!((out.turns, out.tool_calls, out.failed_tool_calls), (2, 1, 0));
        assert!(out.error.is_none());
        let (tool, args, ok, result) = transcript.tool_calls().next().unwrap();
        assert_eq!((tool, ok), ("ontology_entity", true));
        assert_eq!(args["id"], "concept:customer");
        assert_eq!(result["found"], true);
        assert_eq!(transcript.agent_texts().collect::<Vec<_>>(), vec!["Let me look.", "A customer is an account holder."]);
    }

    #[test]
    fn a_server_that_did_not_connect_and_error_results_are_reported() {
        let init = r#"{"type":"system","subtype":"init","session_id":"s1","mcp_servers":[{"name":"onto","status":"failed"}]}"#;
        let err = read_stream([init.to_string()], &["onto".to_string()], &mut Transcript::default()).unwrap_err();
        assert!(err.to_string().contains("did not connect"), "{err}");

        let limited = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":12}"#;
        let out = read_stream([limited.to_string()], &[], &mut Transcript::default()).unwrap();
        assert_eq!(out.error.as_deref(), Some("error_max_turns"));
    }
}
