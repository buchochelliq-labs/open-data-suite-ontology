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

use crate::human::Person;
use crate::session::Session;
use crate::transcript::{Event, Transcript};

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
    /// Space requests to stay under a provider's per-minute limit (e.g. 5 on
    /// Gemini's free tier) instead of hitting it and backing off.
    pub requests_per_minute: Option<u32>,
    /// A cap on requests for the whole run, shared by every clone of this
    /// config (agent and judge), for providers with small daily quotas.
    pub budget: Option<std::sync::Arc<RequestBudget>>,
}

/// Requests a run may still make.
#[derive(Debug)]
pub struct RequestBudget {
    max: usize,
    used: std::sync::atomic::AtomicUsize,
}

impl RequestBudget {
    pub fn new(max: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { max, used: Default::default() })
    }

    pub fn used(&self) -> usize {
        self.used.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Take one request, or fail when none are left.
    fn take(&self) -> Result<(), BudgetExhausted> {
        let previous = self.used.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if previous >= self.max {
            self.used.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return Err(BudgetExhausted(format!("the run's budget of {} requests is used", self.max)));
        }
        Ok(())
    }
}

/// No more requests can succeed today (a provider's daily quota, or the run's
/// own budget). Retrying cannot help, so callers stop the whole run.
#[derive(Debug)]
pub struct BudgetExhausted(pub String);

impl std::fmt::Display for BudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request budget exhausted: {}", self.0)
    }
}

impl std::error::Error for BudgetExhausted {}

/// Whether `error` means no further requests can succeed in this run.
pub fn is_budget_exhausted(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<BudgetExhausted>())
}

impl ModelConfig {
    /// `ONTO_LLM_BASE_URL` (required), `ONTO_LLM_MODEL`, `ONTO_LLM_API_KEY`,
    /// `ONTO_LLM_RPM` (requests per minute), `ONTO_LLM_MAX_REQUESTS` and
    /// `ONTO_LLM_TIMEOUT_SECS`.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("ONTO_LLM_BASE_URL").ok().filter(|v| !v.is_empty())?;
        Some(Self {
            base_url,
            model: std::env::var("ONTO_LLM_MODEL").unwrap_or_else(|_| "local".into()),
            api_key: std::env::var("ONTO_LLM_API_KEY").ok().filter(|v| !v.is_empty()),
            max_turns: 16,
            max_tool_result_chars: 4_000,
            timeout: Duration::from_secs(env_number("ONTO_LLM_TIMEOUT_SECS").unwrap_or(600)),
            requests_per_minute: env_number("ONTO_LLM_RPM").map(|n| n as u32).filter(|n| *n > 0),
            budget: env_number("ONTO_LLM_MAX_REQUESTS").map(|n| RequestBudget::new(n as usize)),
        })
    }
}

/// What the simulated person does when the agent stops to talk.
#[derive(Clone, Debug)]
pub enum Reply {
    Say(String),
    /// Accept the most recent successful proposal, then tell the agent (and
    /// say `then`, if anything).
    AcceptLastProposal {
        by: String,
        then: Option<String>,
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

    /// The next scripted reply, or `End` when the script is exhausted.
    pub fn next_reply(&mut self) -> Reply {
        self.replies.pop_front().unwrap_or(Reply::End)
    }
}

#[derive(Debug, Default)]
pub struct EpisodeStats {
    pub turns: usize,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
}

/// The digest of the most recent successful `ontology_propose`.
pub(crate) fn last_proposal(transcript: &Transcript) -> Option<String> {
    transcript
        .tool_calls()
        .filter(|(tool, _, ok, _)| *tool == "ontology_propose" && *ok)
        .filter_map(|(.., result)| result["digest"].as_str().map(str::to_string))
        .last()
}

fn truncate(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}\n[truncated: narrow the request for more]")
}

fn env_number(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// When each endpoint may next be called. Shared by the agent and the judge,
/// which usually spend the same key's quota.
static NEXT_SLOT: std::sync::LazyLock<tokio::sync::Mutex<std::collections::HashMap<String, tokio::time::Instant>>> = std::sync::LazyLock::new(Default::default);

/// Wait for this endpoint's next request slot under `config.requests_per_minute`.
async fn pace(config: &ModelConfig) {
    let Some(rpm) = config.requests_per_minute else { return };
    let interval = Duration::from_secs(60) / rpm;
    let mut slots = NEXT_SLOT.lock().await;
    let now = tokio::time::Instant::now();
    let slot = slots.get(&config.base_url).copied().filter(|t| *t > now).unwrap_or(now);
    slots.insert(config.base_url.clone(), slot + interval);
    drop(slots);
    tokio::time::sleep_until(slot).await;
}

/// Longest a single MCP tool call may take before it counts as failed.
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

/// Print one line of the live conversation log to stderr, unless
/// `ONTO_LOG_QUIET` is set. In CI this is what shows the run's progress.
pub fn chat(line: String) {
    if std::env::var_os("ONTO_LOG_QUIET").is_none() {
        for (i, part) in line.lines().enumerate() {
            eprintln!("{}{part}", if i == 0 { "    " } else { "      " });
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { format!("{}… ({} chars)", text.chars().take(max).collect::<String>(), text.chars().count()) }
}

/// Attempts per request when the endpoint is rate-limited or briefly unavailable.
const ATTEMPTS: u32 = 6;
/// Longest single wait, whatever `Retry-After` asks for.
const MAX_WAIT: Duration = Duration::from_secs(120);

/// A rate-limit response caused by a per-day quota (Google reports quota ids
/// such as `GenerateRequestsPerDayPerProjectPerModel-FreeTier`).
fn is_daily_quota(body: &str) -> bool {
    body.contains("PerDay")
}

/// Seconds from a Google `RetryInfo` detail, e.g. `"retryDelay": "14s"`.
fn retry_delay_secs(body: &str) -> Option<u64> {
    let after = &body[body.find("retryDelay")? + "retryDelay".len()..];
    let digits: String = after.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

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
        pace(config).await;
        let mut request = http.post(&url).json(&body);
        if let Some(key) = &config.api_key {
            request = request.bearer_auth(key);
        }
        if let Some(budget) = &config.budget {
            budget.take()?;
        }
        let started = std::time::Instant::now();
        let response = match request.send().await {
            Ok(response) => response,
            // A provider that does not answer within the timeout is treated like an overload.
            Err(e) if e.is_timeout() && attempt < ATTEMPTS => {
                let wait = Duration::from_secs(2u64.pow(attempt)).min(MAX_WAIT);
                chat(format!("· model did not answer within {}s; retrying in {}s (attempt {attempt}/{ATTEMPTS})", config.timeout.as_secs(), wait.as_secs()));
                tokio::time::sleep(wait).await;
                continue;
            }
            Err(e) => return Err(e).context("calling the model"),
        };
        let status = response.status();
        let retry_after = response.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        let text = response.text().await.context("reading the model response")?;
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && is_daily_quota(&text) {
            return Err(BudgetExhausted(format!("the provider's daily quota is used ({})", text.chars().take(300).collect::<String>())).into());
        }
        if retryable(status) && attempt < ATTEMPTS {
            let wait = retry_after
                .or_else(|| retry_delay_secs(&text))
                .map(|secs| Duration::from_secs(secs + 1))
                .unwrap_or_else(|| Duration::from_secs(2u64.pow(attempt)))
                .min(MAX_WAIT);
            chat(format!(
                "· model returned {status} after {:.1}s ({}); retrying in {}s (attempt {attempt}/{ATTEMPTS})",
                started.elapsed().as_secs_f64(),
                clip(text.trim(), 160),
                wait.as_secs()
            ));
            tokio::time::sleep(wait).await;
            continue;
        }
        if !status.is_success() {
            bail!("model returned {status}: {}", text.chars().take(500).collect::<String>());
        }
        let body: Value = serde_json::from_str(&text).context("decoding the model response")?;
        return body["choices"][0]["message"].as_object().cloned().map(Value::Object).with_context(|| format!("no message in {body}"));
    }
    unreachable!("the last attempt returns or bails")
}

/// A tool's input schema without the `$schema` dialect marker, which some
/// OpenAI-compatible endpoints reject in function parameters.
fn portable_schema(schema: &serde_json::Map<String, Value>) -> Value {
    let mut schema = schema.clone();
    schema.remove("$schema");
    Value::Object(schema)
}

/// Run one conversation: `opening` from the person, then alternate model turns
/// (with tool calls) and the person's replies until they end it or `max_turns`.
pub async fn run_episode(session: &mut Session, config: &ModelConfig, system_prompt: &str, opening: &str, human: &mut impl Person) -> Result<EpisodeStats> {
    let http = reqwest::Client::builder().timeout(config.timeout).connect_timeout(Duration::from_secs(30)).build()?;
    let tools: Vec<Value> = session
        .tools()
        .await?
        .into_iter()
        .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": portable_schema(&t.input_schema) } }))
        .collect();
    let system = match session.instructions() {
        Some(instructions) => format!("{system_prompt}\n\n{instructions}"),
        None => system_prompt.to_string(),
    };
    let mut messages = vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": opening })];
    session.human_says(opening);
    chat(format!("USER: {opening}"));
    let mut stats = EpisodeStats::default();

    while stats.turns < config.max_turns {
        stats.turns += 1;
        chat(format!("· model request {} ({} messages, {} tools)", stats.turns, messages.len(), tools.len()));
        let started = std::time::Instant::now();
        let message = complete(&http, config, &messages, &tools).await?;
        chat(format!("· model replied in {:.1}s", started.elapsed().as_secs_f64()));
        let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
        let text = message["content"].as_str().unwrap_or_default().trim().to_string();
        messages.push(message);

        if !calls.is_empty() {
            if !text.is_empty() {
                chat(format!("ASSISTANT: {text}"));
            }
            for call in calls {
                let name = call["function"]["name"].as_str().unwrap_or_default().to_string();
                let raw = call["function"]["arguments"].as_str().unwrap_or("{}");
                chat(format!("ASSISTANT → {name} {raw}"));
                stats.tool_calls += 1;
                let content = match serde_json::from_str::<Value>(if raw.trim().is_empty() { "{}" } else { raw }) {
                    Ok(arguments) => {
                        let started = std::time::Instant::now();
                        let (ok, result) = match tokio::time::timeout(TOOL_TIMEOUT, session.call(&name, arguments.clone())).await {
                            Ok(outcome) => outcome?,
                            Err(_) => {
                                let message = format!("tool call timed out after {}s", TOOL_TIMEOUT.as_secs());
                                session.transcript.events.push(Event::ToolCall {
                                    tool: name.clone(),
                                    arguments,
                                    ok: false,
                                    result: Value::String(message.clone()),
                                });
                                (false, Value::String(message))
                            }
                        };
                        if !ok {
                            stats.failed_tool_calls += 1;
                        }
                        let body = if ok { result.to_string() } else { format!("error: {result}") };
                        chat(format!("TOOL {name} {} in {:.1}s: {}", if ok { "ok" } else { "FAILED" }, started.elapsed().as_secs_f64(), clip(&body, 600)));
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
                        chat(format!("TOOL {name} FAILED: arguments are not valid JSON: {e}"));
                        format!("error: arguments are not valid JSON: {e}")
                    }
                };
                messages.push(json!({ "role": "tool", "tool_call_id": call["id"], "name": name, "content": content }));
            }
            continue;
        }

        chat(format!("ASSISTANT: {}", if text.is_empty() { "(empty reply)" } else { &text }));
        session.agent_says(&text);
        let reply = match human.reply(&session.transcript).await? {
            Reply::Say(text) => text,
            Reply::AcceptLastProposal { by, then } => {
                let accepted = match last_proposal(&session.transcript) {
                    Some(digest) => {
                        session.human_accepts(&digest, &by)?;
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
        session.human_says(&reply);
        messages.push(json!({ "role": "user", "content": reply }));
    }
    if stats.turns >= config.max_turns {
        chat(format!("· stopped after {} model turns", config.max_turns));
    }
    Ok(stats)
}
