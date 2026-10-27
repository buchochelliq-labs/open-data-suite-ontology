use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::Session;

/// One thing that happened in an episode. Transcripts serialize as JSON lines
/// so model runs can be recorded and replayed as deterministic tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The person's message to the agent.
    Human {
        text: String,
    },
    /// The agent's message to the person.
    Agent {
        text: String,
    },
    ToolCall {
        tool: String,
        arguments: Value,
        ok: bool,
        result: Value,
    },
    /// Something the person did outside the conversation (e.g. accepting a proposal).
    HumanAction {
        action: String,
        detail: Value,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub events: Vec<Event>,
}

impl Transcript {
    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &Value, bool, &Value)> {
        self.events.iter().filter_map(|e| match e {
            Event::ToolCall { tool, arguments, ok, result } => Some((tool.as_str(), arguments, *ok, result)),
            _ => None,
        })
    }

    pub fn called(&self, tool: &str) -> bool {
        self.tool_calls().any(|(t, ..)| t == tool)
    }

    pub fn agent_texts(&self) -> impl Iterator<Item = &str> {
        self.events.iter().filter_map(|e| match e {
            Event::Agent { text } => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn to_jsonl(&self) -> String {
        self.events.iter().map(|e| serde_json::to_string(e).expect("serializable event") + "\n").collect()
    }

    pub fn from_jsonl(text: &str) -> serde_json::Result<Self> {
        let events = text.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<Result<_, _>>()?;
        Ok(Self { events })
    }
}

/// Fields that legitimately differ between runs.
const VOLATILE: &[&str] = &["retrieved_at", "synced_at", "source_version", "version", "created_at", "accepted_at", "score"];

pub fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map.iter().filter(|(k, _)| !VOLATILE.contains(&k.as_str())).map(|(k, v)| (k.clone(), normalize(v))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

/// Re-run every recorded tool call and human action against `session` and
/// report calls whose outcome differs from the recording.
pub async fn replay(session: &mut Session, recorded: &Transcript) -> anyhow::Result<Vec<String>> {
    let mut mismatches = Vec::new();
    for event in &recorded.events {
        match event {
            Event::ToolCall { tool, arguments, ok, result } => {
                let (now_ok, now_result) = session.call(tool, arguments.clone()).await?;
                if now_ok != *ok || normalize(&now_result) != normalize(result) {
                    mismatches.push(format!("{tool} {arguments}: recorded ok={ok} {result}\n  now ok={now_ok} {now_result}"));
                }
            }
            Event::HumanAction { action, detail } if action == "accept" => {
                let digest = detail["digest"].as_str().unwrap_or_default();
                session.human_accepts(digest, detail["by"].as_str().unwrap_or("replay"))?;
            }
            Event::Human { text } => session.transcript.events.push(Event::Human { text: text.clone() }),
            Event::Agent { text } => session.transcript.events.push(Event::Agent { text: text.clone() }),
            Event::HumanAction { .. } => {}
        }
    }
    Ok(mismatches)
}
