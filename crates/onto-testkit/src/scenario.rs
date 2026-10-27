//! Scripted scenarios: deterministic agent/human scripts in YAML.
//!
//! ```yaml
//! name: conflicting definition is surfaced
//! fixture: shop
//! steps:
//!   - call: ontology_entity
//!     args: { id: concept:customer }
//!     expect:
//!       - { path: /found, equals: true }
//!       - { path: /entity, contains: "Finance only counts" }
//!   - human: "Use the finance definition."
//!   - call: ontology_propose
//!     args: { ... }
//!     save_as: proposal
//!   - accept: proposal          # the person accepts, via the CLI path
//! ```

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::fixture::Fixture;
use crate::invariants;
use crate::session::Session;
use crate::transcript::Transcript;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub fixture: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Call {
        call: String,
        #[serde(default)]
        args: Value,
        /// Whether the tool call should succeed (default true).
        #[serde(default = "yes")]
        ok: bool,
        #[serde(default)]
        expect: Vec<Expect>,
        #[serde(default)]
        save_as: Option<String>,
    },
    Human {
        human: String,
    },
    Agent {
        agent: String,
    },
    Accept {
        /// Name given to an earlier `save_as` proposal result.
        accept: String,
        #[serde(default = "reviewer")]
        by: String,
    },
}

fn yes() -> bool {
    true
}

fn reviewer() -> String {
    "scenario-reviewer".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// JSON pointer into the tool result, e.g. `/entity/properties/0`.
    pub path: String,
    #[serde(default)]
    pub equals: Option<Value>,
    /// Substring of the serialized value at `path`.
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub not_contains: Option<String>,
}

impl Expect {
    fn check(&self, result: &Value) -> Result<()> {
        let value = result.pointer(&self.path).with_context(|| format!("no value at {} in {result}", self.path))?;
        if let Some(expected) = &self.equals
            && value != expected
        {
            bail!("{}: expected {expected}, got {value}", self.path);
        }
        let text = value.to_string();
        if let Some(needle) = &self.contains
            && !text.contains(needle)
        {
            bail!("{}: expected to contain {needle:?}, got {text}", self.path);
        }
        if let Some(needle) = &self.not_contains
            && text.contains(needle)
        {
            bail!("{}: expected not to contain {needle:?}, got {text}", self.path);
        }
        Ok(())
    }
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_norway::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Run against a fresh fixture, checking expectations and invariants.
    pub async fn run(&self) -> Result<Transcript> {
        let fixture = Fixture::new(&self.fixture)?;
        let before = fixture.repository_files()?;
        let mut session = Session::start(&fixture).await?;
        let mut saved: HashMap<String, Value> = HashMap::new();

        for (index, step) in self.steps.iter().enumerate() {
            let at = || format!("{}: step {}", self.name, index + 1);
            match step {
                Step::Call { call, args, ok, expect, save_as } => {
                    let (got_ok, result) = session.call(call, args.clone()).await.with_context(at)?;
                    if got_ok != *ok {
                        bail!("{}: {call} returned ok={got_ok}, expected {ok}: {result}", at());
                    }
                    for e in expect {
                        e.check(&result).with_context(at)?;
                    }
                    if let Some(name) = save_as {
                        saved.insert(name.clone(), result);
                    }
                }
                Step::Human { human } => session.human_says(human),
                Step::Agent { agent } => session.agent_says(agent),
                Step::Accept { accept, by } => {
                    let digest =
                        saved.get(accept).and_then(|v| v["digest"].as_str()).with_context(|| format!("{}: no saved proposal `{accept}`", at()))?.to_string();
                    session.human_accepts(&digest, by).with_context(at)?;
                }
            }
        }

        let transcript = session.close().await?;
        invariants::check(&transcript, &before, &fixture.repository_files()?).assert_clean();
        Ok(transcript)
    }
}
