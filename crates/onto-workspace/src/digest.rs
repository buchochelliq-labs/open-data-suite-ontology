//! A compact Markdown digest of the team's agreed context, for agents.
//!
//! MCP tools only help when the agent thinks to call them. The digest puts the
//! facts most often needed in front of it: concepts, standards and decisions,
//! the team's facts about models, and open conflicts, each with its source and
//! authority. Facts that only OpenDataSuite asserts are left out (agents read
//! those through ODS); a conflict with them is kept. The digest has a size
//! budget; what does not fit is counted, and the agent is pointed to the tools.

use std::collections::BTreeSet;

use onto_core::{Conflict, ConflictStatus, EntityView, Object, Property, Value, vocab};
use onto_store::{ProposalStatus, Store};

use crate::config::SourceKind;
use crate::proposals;
use crate::workspace::{Result, Workspace};

#[derive(Clone, Debug)]
pub struct DigestOptions {
    /// Longest digest in characters (about four per token).
    pub max_chars: usize,
}

impl Default for DigestOptions {
    fn default() -> Self {
        Self { max_chars: 16_000 }
    }
}

/// Sections in the order they are filled, most useful first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
    Standards,
    Concepts,
    Assets,
    Columns,
    Other,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Standards => "Standards and decisions",
            Section::Concepts => "Concepts",
            Section::Assets => "Models and other assets",
            Section::Columns => "Columns",
            Section::Other => "Other",
        }
    }

    fn of(view: &EntityView) -> Option<Section> {
        let types = view.entities(vocab::TYPE);
        if types.iter().any(|t| *t == vocab::TYPE_TYPE || *t == vocab::PREDICATE_TYPE) {
            return None;
        }
        Some(if types.iter().any(|t| *t == "ModellingStandard" || *t == "Decision") {
            Section::Standards
        } else if types.contains(&"BusinessConcept") {
            Section::Concepts
        } else if view.id.contains('#') || types.contains(&"Attribute") {
            Section::Columns
        } else if types.contains(&"DataAsset") || view.id.starts_with("model.") || view.id.starts_with("source.") {
            Section::Assets
        } else {
            Section::Other
        })
    }
}

fn snake<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn object(object: &Object) -> String {
    match object {
        Object::Entity(id) => format!("`{id}`"),
        Object::Literal(serde_json::Value::String(s)) => s.clone(),
        Object::Literal(other) => other.to_string(),
    }
}

fn cite(value: &Value) -> String {
    let sources: Vec<String> = value.sources.iter().map(|p| format!("{}, {}", p.source_id, snake(&p.authority))).collect();
    format!("_({})_", sources.join("; "))
}

fn qualifiers(value: &Value) -> String {
    if value.qualifiers.is_empty() {
        return String::new();
    }
    let parts: Vec<String> =
        value.qualifiers.iter().map(|(k, v)| format!("{k}: {}", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect();
    format!(" ({})", parts.join(", "))
}

struct Renderer<'a> {
    /// Sources whose facts the digest shows: everything but ODS.
    team: &'a BTreeSet<String>,
}

impl Renderer<'_> {
    fn is_team(&self, value: &Value) -> bool {
        value.sources.iter().any(|p| self.team.contains(&p.source_id))
    }

    fn worth_listing(&self, view: &EntityView) -> bool {
        view.properties.iter().any(|p| {
            p.values.iter().any(|v| self.is_team(v))
                || !p.pending.is_empty()
                || p.conflict.as_ref().is_some_and(|c| c.candidates.iter().any(|v| self.is_team(v)))
        })
    }

    fn conflict(&self, property: &Property, conflict: &Conflict) -> String {
        let candidates: Vec<String> = conflict.candidates.iter().map(|v| format!("\"{}\" {}", object(&v.object), cite(v))).collect();
        let status = match &conflict.status {
            ConflictStatus::ResolvedByPriority { selected_source } => format!("settled by source priority in favour of {selected_source}"),
            ConflictStatus::Unresolved => "UNRESOLVED: ask the user".to_string(),
        };
        let predicate = property.predicate.strip_prefix("onto:").unwrap_or(&property.predicate);
        format!("  - ⚠ sources disagree on {predicate}: {} ({status})\n", candidates.join(" vs "))
    }

    fn entity(&self, view: &EntityView) -> String {
        let label = view.text(vocab::LABEL);
        let mut out = match label {
            Some(label) if label != view.id => format!("- **{label}** `{}`", view.id),
            _ => format!("- `{}`", view.id),
        };
        if let Some(definition) = view.property(vocab::DEFINITION).and_then(|p| p.values.first()) {
            out.push_str(&format!(": {} {}", object(&definition.object), cite(definition)));
        }
        out.push('\n');
        let aliases: Vec<String> = view.property(vocab::ALIAS).map(|p| p.values.iter().map(|v| object(&v.object)).collect()).unwrap_or_default();
        if !aliases.is_empty() {
            out.push_str(&format!("  - also called: {}\n", aliases.join(", ")));
        }
        for property in &view.properties {
            if let Some(conflict) = &property.conflict {
                out.push_str(&self.conflict(property, conflict));
            }
            if [vocab::LABEL, vocab::DEFINITION, vocab::ALIAS, vocab::TYPE].contains(&property.predicate.as_str()) {
                continue;
            }
            for value in property.values.iter().filter(|v| self.is_team(v)) {
                let arrow = if value.object.entity().is_some() { " →" } else { ":" };
                out.push_str(&format!("  - {}{arrow} {}{} {}\n", property.predicate, object(&value.object), qualifiers(value), cite(value)));
            }
            for value in &property.pending {
                out.push_str(&format!("  - pending proposal, not a fact: {} = {}\n", property.predicate, object(&value.object)));
            }
        }
        out
    }
}

/// Render the digest for `workspace` from `store`.
pub fn render(workspace: &Workspace, store: &Store, options: &DigestOptions) -> Result<String> {
    let sources = store.sources()?;
    let team: BTreeSet<String> = sources
        .iter()
        .filter(|s| !matches!(workspace.source_config(&s.source_id).map(|c| &c.kind), Some(SourceKind::Ods { .. })))
        .map(|s| s.source_id.clone())
        .collect();
    let schema = workspace.schema(store)?;
    let resolver = workspace.resolver(Some(crate::now_rfc3339()));
    let renderer = Renderer { team: &team };

    let mut entries: Vec<(Section, String)> = Vec::new();
    for subject in store.subjects()? {
        let mut records = store.records_about(&subject)?;
        records.extend(proposals::pending_records(store, &subject)?);
        let view = resolver.resolve_entity(&subject, &records, &schema);
        let Some(section) = Section::of(&view) else { continue };
        if renderer.worth_listing(&view) {
            entries.push((section, renderer.entity(&view)));
        }
    }
    entries.sort_by_key(|(section, _)| *section);

    let mut out = String::from(
        "# Team context\n\nThe team's agreed facts from the ontology, each with its source and authority \
         (reviewed and declared are agreed; observed is read from an artifact; inferred is a guess). \
         Facts that only the dbt project shows are not repeated here: ask OpenDataSuite for structure. \
         For anything missing, use the ontology tools.\n\nSources: ",
    );
    let listed: Vec<String> =
        sources.iter().map(|s| format!("{} ({}, {})", s.source_id, snake(&s.layer), s.version.as_deref().unwrap_or("unversioned"))).collect();
    out.push_str(&listed.join("; "));
    out.push('\n');
    let pending = store.proposals(Some(ProposalStatus::Pending))?.len();
    if pending > 0 {
        out.push_str(&format!("\n{pending} proposal(s) await review; they are not facts.\n"));
    }

    let mut omitted = 0;
    let mut current = None;
    for (section, text) in &entries {
        let heading = if current != Some(*section) { format!("\n## {}\n\n", section.title()) } else { String::new() };
        if out.len() + heading.len() + text.len() > options.max_chars {
            omitted += 1;
            continue;
        }
        out.push_str(&heading);
        out.push_str(text);
        current = Some(*section);
    }
    if omitted > 0 {
        out.push_str(&format!("\n_{omitted} more entities did not fit; use `ontology_search` and `ontology_entity` for them._\n"));
    }
    Ok(out)
}
