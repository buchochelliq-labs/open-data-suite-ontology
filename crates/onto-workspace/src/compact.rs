//! One-line facts for agents: what an entity says, with each value's source
//! and authority, without the full provenance records.
//!
//! Full provenance (locators, versions, retrieval times) is most of an
//! entity's size and rarely changes an answer; agents can still ask for it.

use std::collections::BTreeSet;

use onto_core::{ConflictStatus, EntityView, Object, Value, vocab};
use onto_store::Store;

use crate::config::SourceKind;
use crate::workspace::{Result, Workspace};

/// Ids of the sources whose facts are the team's own: every source except
/// OpenDataSuite, whose facts agents can read through ODS itself.
pub fn team_sources(workspace: &Workspace, store: &Store) -> Result<BTreeSet<String>> {
    Ok(store
        .sources()?
        .into_iter()
        .filter(|s| !matches!(workspace.source_config(&s.source_id).map(|c| &c.kind), Some(SourceKind::Ods { .. })))
        .map(|s| s.source_id)
        .collect())
}

/// Whether the workspace reads a dbt project through OpenDataSuite.
pub fn has_ods_source(workspace: &Workspace) -> bool {
    workspace.config.sources.iter().any(|s| matches!(s.kind, SourceKind::Ods { .. }))
}

/// A predicate as people read it: reserved `onto:` predicates without the prefix.
pub fn predicate(name: &str) -> &str {
    name.strip_prefix("onto:").unwrap_or(name)
}

pub fn object(object: &Object) -> String {
    match object {
        Object::Entity(id) => id.clone(),
        Object::Literal(serde_json::Value::String(s)) => s.clone(),
        Object::Literal(other) => other.to_string(),
    }
}

fn snake<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// `[repo reviewed by data-team-lead; central declared]`
pub fn sources(value: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in &value.sources {
        let part = match &p.attributed_to {
            Some(by) => format!("{} {} by {by}", p.source_id, snake(&p.authority)),
            None => format!("{} {}", p.source_id, snake(&p.authority)),
        };
        // The same source may assert a fact in more than one file.
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    format!("[{}]", parts.join("; "))
}

fn qualifiers(value: &Value) -> String {
    if value.qualifiers.is_empty() {
        return String::new();
    }
    let parts: Vec<String> =
        value.qualifiers.iter().map(|(k, v)| format!("{k}: {}", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect();
    format!(" ({})", parts.join(", "))
}

/// `grain: one row per order [repo reviewed]`, or `represents → concept:order [...]`.
pub fn line(predicate_name: &str, value: &Value) -> String {
    let separator = if value.object.entity().is_some() { " →" } else { ":" };
    format!("{}{separator} {}{} {}", predicate(predicate_name), object(&value.object), qualifiers(value), sources(value))
}

/// An entity's facts as lines, and how many values `keep` left out.
pub struct Facts {
    pub lines: Vec<String>,
    pub omitted: usize,
}

/// Lines for every kept value, every conflict (all candidates, kept or not:
/// a disagreement matters even with a source the caller leaves out) and every
/// pending proposal. Label and type are left to the caller; relationships
/// (entity values) are included only when `relations` is set.
pub fn facts(view: &EntityView, relations: bool, keep: impl Fn(&Value) -> bool) -> Facts {
    let mut lines = Vec::new();
    let mut omitted = 0;
    for property in &view.properties {
        if property.predicate == vocab::LABEL || property.predicate == vocab::TYPE {
            continue;
        }
        let name = predicate(&property.predicate);
        if let Some(conflict) = &property.conflict {
            let candidates: Vec<String> = conflict.candidates.iter().map(|v| format!("\"{}\" {}", object(&v.object), sources(v))).collect();
            let status = match &conflict.status {
                ConflictStatus::ResolvedByPriority { selected_source } => format!("settled by source priority in favour of {selected_source}"),
                ConflictStatus::Unresolved => "UNRESOLVED, ask the user".to_string(),
            };
            lines.push(format!("{name}: sources disagree ({status}): {}", candidates.join(" vs ")));
        }
        for value in &property.values {
            if property.conflict.is_some() || (!relations && value.object.entity().is_some()) {
                continue;
            }
            if keep(value) {
                lines.push(line(&property.predicate, value));
            } else {
                omitted += 1;
            }
        }
        for value in &property.pending {
            lines.push(format!("{name}: PENDING proposal, not a fact: {}", object(&value.object)));
        }
    }
    Facts { lines, omitted }
}

/// A one-line summary for search hits: types, definition and grain, shortened.
pub fn summary(view: &EntityView) -> Option<String> {
    let clip = |s: &str, n: usize| if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() };
    let mut parts = Vec::new();
    if let Some(definition) = view.text(vocab::DEFINITION) {
        parts.push(clip(definition, 140));
    }
    if let Some(grain) = view.text("grain") {
        parts.push(format!("grain: {}", clip(grain, 80)));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}
