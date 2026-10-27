//! Model-driven episodes (feature `llm`).
//!
//! The agent is any model behind an OpenAI-compatible `/chat/completions`
//! endpoint with tool calling: `llama-server` with a small GGUF model in CI,
//! or a hosted model for deeper evaluations. MCP tools are offered as functions
//! and every call goes through the real [`Session`], so the same invariants
//! apply as for scripted scenarios.

use std::collections::VecDeque;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::session::Session;
use crate::transcript::Event;

#[derive(Clone, Debug)]
pub struct ModelConfig {
    /// Base URL including `/v1`, e.g. `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub max_turns: usize,
    /// Tool results are truncated to this many characters to fit small contexts.
    pub max_tool_result_chars: usize,
    pub timeout: Duration,
}

impl ModelConfig {
    /// `ONTO_LLM_BASE_URL` (required), `ONTO_LLM_MODEL`, `ONTO_LLM_API_KEY`.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("ONTO_LLM_BASE_URL").ok().filter(|v| !v.is_empty())?;
        Some(Self {
            base_url,
            model: std::env::var("ONTO_LLM_MODEL").unwrap_or_else(|_| "local".into()),
            api_key: std::env::var("ONTO_LLM_API_KEY").ok().filter(|v| !v.is_empty()),
            max_turns: 16,
            max_tool_result_chars: 4_000,
            timeout: Duration::from_secs(600),
        })
    }
}

/// What the simulated person does when the agent stops to talk.
#[derive(Clone, Debug)]
pub enum Reply {
    Say(String),
    /// Accept the most recent successful proposal, then tell the agent.
    AcceptLastProposal {
        by: String,
    },
    End,
}

/// A person following a script. Once the script runs out, the episode ends.
#[derive(Clone, Debug, Default)]
pub struct ScriptedHuman {
    replies: VecDeque<Reply>,
}

impl ScriptedHuman {
    pub fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self { replies: replies.into_iter().collect() }
    }

    fn next(&mut self) -> Reply {
        self.replies.pop_front().unwrap_or(Reply::End)
    }
}

#[derive(Debug, Default)]
pub struct EpisodeStats {
    pub turns: usize,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
}

fn truncate(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}\n[truncated: narrow the request for more]")
}

/// Attempts per request when the endpoint is rate-limited or briefly unavailable.
const ATTEMPTS: u32 = 6;
/// Longest single wait, whatever `Retry-After` asks for.
const MAX_WAIT: Duration = Duration::from_secs(120);

fn retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || matches!(status.as_u16(), 500 | 502 | 503 | 504)
}

/// One chat completion; returns the assistant message. Tools are omitted when
/// empty. Free tiers (e.g. GitHub Models) rate-limit, so 429 and transient 5xx
/// responses are retried, honouring `Retry-After` and otherwise backing off.
pub(crate) async fn complete(http: &reqwest::Client, config: &ModelConfig, messages: &[Value], tools: &[Value]) -> Result<Value> {
    let mut body = json!({ "model": config.model, "messages": messages, "temperature": 0 });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    for attempt in 1..=ATTEMPTS {
        let mut request = http.post(&url).json(&body);
        if let Some(key) = &config.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.context("calling the model")?;
        let status = response.status();
        if retryable(status) && attempt < ATTEMPTS {
            let wait = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or_else(|| Duration::from_secs(2u64.pow(attempt)))
                .min(MAX_WAIT);
            eprintln!("model returned {status}; retrying in {}s (attempt {attempt}/{ATTEMPTS})", wait.as_secs());
            tokio::time::sleep(wait).await;
            continue;
        }
        let text = response.text().await.context("reading the model response")?;
        if !status.is_success() {
            bail!("model returned {status}: {}", text.chars().take(500).collect::<String>());
        }
        let body: Value = serde_json::from_str(&text).context("decoding the model response")?;
        return body["choices"][0]["message"].as_object().cloned().map(Value::Object).with_context(|| format!("no message in {body}"));
    }
    unreachable!("the last attempt returns or bails")
}

/// Run one conversation: `opening` from the person, then alternate model turns
/// (with tool calls) and scripted replies until the script ends or `max_turns`.
pub async fn run_episode(session: &mut Session, config: &ModelConfig, system_prompt: &str, opening: &str, human: &mut ScriptedHuman) -> Result<EpisodeStats> {
    let http = reqwest::Client::builder().timeout(config.timeout).build()?;
    let tools: Vec<Value> = session
        .tools()
        .await?
        .into_iter()
        .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": *t.input_schema } }))
        .collect();
    let system = match session.instructions() {
        Some(instructions) => format!("{system_prompt}\n\n{instructions}"),
        None => system_prompt.to_string(),
    };
    let mut messages = vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": opening })];
    session.human_says(opening);
    let mut stats = EpisodeStats::default();

    while stats.turns < config.max_turns {
        stats.turns += 1;
        let message = complete(&http, config, &messages, &tools).await?;
        let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
        let text = message["content"].as_str().unwrap_or_default().trim().to_string();
        messages.push(message);

        if !calls.is_empty() {
            for call in calls {
                let name = call["function"]["name"].as_str().unwrap_or_default().to_string();
                let raw = call["function"]["arguments"].as_str().unwrap_or("{}");
                stats.tool_calls += 1;
                let content = match serde_json::from_str::<Value>(if raw.trim().is_empty() { "{}" } else { raw }) {
                    Ok(arguments) => {
                        let (ok, result) = session.call(&name, arguments).await?;
                        if !ok {
                            stats.failed_tool_calls += 1;
                        }
                        let body = if ok { result.to_string() } else { format!("error: {result}") };
                        truncate(body, config.max_tool_result_chars)
                    }
                    Err(e) => {
                        stats.failed_tool_calls += 1;
                        session.transcript.events.push(Event::ToolCall {
                            tool: name.clone(),
                            arguments: Value::String(raw.to_string()),
                            ok: false,
                            result: Value::String(e.to_string()),
                        });
                        format!("error: arguments are not valid JSON: {e}")
                    }
                };
                messages.push(json!({ "role": "tool", "tool_call_id": call["id"], "name": name, "content": content }));
            }
            continue;
        }

        session.agent_says(&text);
        let reply = match human.next() {
            Reply::Say(text) => text,
            Reply::AcceptLastProposal { by } => {
                let digest = session
                    .transcript
                    .tool_calls()
                    .filter(|(tool, _, ok, _)| *tool == "ontology_propose" && *ok)
                    .filter_map(|(.., result)| result["digest"].as_str().map(str::to_string))
                    .last();
                match digest {
                    Some(digest) => {
                        session.human_accepts(&digest, &by)?;
                        format!("I accepted proposal {digest} with the onto CLI.")
                    }
                    None => "I don't see a proposal to accept.".to_string(),
                }
            }
            Reply::End => break,
        };
        session.human_says(&reply);
        messages.push(json!({ "role": "user", "content": reply }));
    }
    Ok(stats)
}
