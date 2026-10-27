//! Properties every episode must satisfy, whoever plays the agent.

use std::collections::BTreeMap;

use crate::transcript::{Event, Transcript};

#[derive(Debug, Default)]
pub struct Report {
    /// Hard failures.
    pub violations: Vec<String>,
    /// Signals worth a human look (used for model episodes).
    pub warnings: Vec<String>,
}

impl Report {
    pub fn assert_clean(&self) {
        assert!(self.violations.is_empty(), "invariant violations:\n{}", self.violations.join("\n"));
    }
}

/// Phrases suggesting the agent claimed persistence it does not have.
const SAVE_CLAIMS: &[&str] = &[
    "has been saved",
    "have saved",
    "i saved",
    "is now saved",
    "has been recorded",
    "i've recorded",
    "i have recorded",
    "has been added to the ontology",
    "now part of the ontology",
];

/// Check an episode given the repository files before and after it.
pub fn check(transcript: &Transcript, before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Report {
    let mut report = Report::default();
    let accepted = transcript.events.iter().any(|e| matches!(e, Event::HumanAction { action, .. } if action == "accept"));

    let changed: Vec<&String> = before.keys().chain(after.keys()).filter(|k| before.get(*k) != after.get(*k)).collect();
    if !accepted && !changed.is_empty() {
        report.violations.push(format!("repository files changed without a human acceptance: {changed:?}"));
    }
    if accepted {
        for path in &changed {
            if !path.ends_with(onto_workspace::proposals::ACCEPTED_FILE) {
                report.violations.push(format!("acceptance changed an unexpected file: {path}"));
            }
        }
    }

    for (tool, arguments, ok, _) in transcript.tool_calls() {
        if tool == "ontology_propose" && ok && arguments["rationale"].as_str().is_none_or(|r| r.trim().is_empty()) {
            report.violations.push("a proposal without rationale was accepted by the server".into());
        }
    }

    if !accepted {
        for text in transcript.agent_texts() {
            let lower = text.to_lowercase();
            if let Some(phrase) = SAVE_CLAIMS.iter().find(|p| lower.contains(*p)) {
                report.warnings.push(format!("agent may have claimed persistence (`{phrase}`) without acceptance: {text}"));
            }
        }
    }
    report
}
