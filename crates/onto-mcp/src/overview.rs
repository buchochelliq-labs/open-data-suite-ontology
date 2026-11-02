//! `ontology_overview`: what kinds of things the graph holds and how they
//! connect, for an agent that has no word to search for yet.
//!
//! Every section is a list of one-line strings, capped in length and in count,
//! and the whole answer stays within a character budget. What does not fit is
//! counted in `omitted`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onto_core::{ConflictStatus, EntityView, Object, vocab};
use onto_store::Store;
use onto_workspace::{Workspace, compact, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const OVERVIEW_CHARS: u32 = 8_000;
pub const MAX_OVERVIEW_CHARS: u32 = 20_000;
const MAX_TYPES: usize = 25;
const MAX_PREDICATES: usize = 30;
const MAX_CONCEPTS: usize = 20;
const MAX_STANDARDS: usize = 15;
/// Type pairs shown per predicate; the rest are counted.
const MAX_PAIRS: usize = 3;
const MAX_ALIASES: usize = 5;
const MAX_REPRESENTED_BY: usize = 4;
const TYPE_DEFINITION_CHARS: usize = 120;
const STANDARD_DEFINITION_CHARS: usize = 200;
const SHORT_TEXT_CHARS: usize = 60;
/// How a literal object is shown in a predicate's type pairs.
const TEXT: &str = "text";
const UNTYPED: &str = "untyped";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OverviewRequest {
    /// Character budget for the answer (default 8000, at most 20000).
    #[serde(default)]
    pub max_chars: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Health {
    /// `id: layer layer, kind, version, synced_at, records`; versions shortened to 12 characters.
    pub sources: Vec<String>,
    /// How conflicts are settled, with source priority highest first.
    pub resolution: String,
    pub pending_proposals: usize,
    /// Sources this person has disabled locally; their facts are absent here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_sources: Vec<String>,
    pub sync_scope: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OverviewResponse {
    /// `Type (entities): definition`, most common first; declared types not yet used show `(0)`.
    pub types: Vec<String>,
    /// `predicate: SubjectType → ObjectType (count)`, most used first; `text` is a literal value.
    pub predicates: Vec<String>,
    /// Business concepts with the most relationships:
    /// `id "label" (n links); also called: …; represented by: …`.
    pub concepts: Vec<String>,
    /// Modelling standards and decisions: `id "label": definition [sources]`.
    pub standards: Vec<String>,
    /// Loaded sources and their freshness, as `ontology_status` reports them.
    pub health: Health,
    /// What did not fit and how to get it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<String>,
    pub notice: String,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn clip(text: &str, n: usize) -> String {
    if text.chars().count() > n { format!("{}…", text.chars().take(n).collect::<String>()) } else { text.to_string() }
}

/// Clipped text with the instruction marker of the full text: the phrase may be past the clip.
fn shown(text: &str, n: usize) -> String {
    format!("{}{}", clip(text, n), compact::instruction_marker(text))
}

fn snake<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// An entity's definition, optionally citing its sources, or a note when
/// sources disagree without a winner.
fn definition(view: &EntityView, n: usize, cite: bool) -> Option<String> {
    let property = view.property(vocab::DEFINITION)?;
    if let Some(value) = property.values.first() {
        let text = compact::object(&value.object);
        let clipped = clip(&text, n);
        let marker = compact::instruction_marker(&text);
        return Some(if cite { format!("{clipped} {}{marker}", compact::sources(value)) } else { format!("{clipped}{marker}") });
    }
    property
        .conflict
        .as_ref()
        .filter(|c| c.status == ConflictStatus::Unresolved)
        .map(|_| "sources disagree on the definition (UNRESOLVED; see ontology_entity)".to_string())
}

/// `DataAsset`, `DataAsset/DataEntity`, or `untyped`.
fn type_label(types: Option<&Vec<String>>) -> String {
    match types {
        Some(types) if !types.is_empty() => types.join("/"),
        _ => UNTYPED.to_string(),
    }
}

struct Concept {
    id: String,
    label: Option<String>,
    aliases: Vec<String>,
}

struct Standard {
    id: String,
    label: Option<String>,
    definition: Option<String>,
}

pub fn overview(workspace: &Workspace, store: &Store, health: Health, req: OverviewRequest) -> Result<OverviewResponse, String> {
    let budget = req.max_chars.unwrap_or(OVERVIEW_CHARS).clamp(1_000, MAX_OVERVIEW_CHARS) as usize;
    let schema = workspace.schema(store).map_err(err)?;
    let resolver = workspace.resolver(Some(now_rfc3339()));

    // One pass over every entity: its types, its values and, for concepts and
    // standards, what the overview shows of them. Pending proposals are not facts
    // and are left out.
    let mut declared_types: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut entity_types: HashMap<String, Vec<String>> = HashMap::new();
    let mut values: Vec<(String, String, Option<String>)> = Vec::new();
    let mut concepts = Vec::new();
    let mut standards = Vec::new();
    for subject in store.subjects().map_err(err)? {
        let view = resolver.resolve_entity(&subject, &store.records_about(&subject).map_err(err)?, &schema);
        let mut types: Vec<String> = view.entities(vocab::TYPE).into_iter().map(str::to_string).collect();
        types.sort();
        types.dedup();
        if types.iter().any(|t| t == vocab::TYPE_TYPE) {
            declared_types.insert(subject.clone(), definition(&view, TYPE_DEFINITION_CHARS, false));
            continue;
        }
        if types.iter().any(|t| t == vocab::PREDICATE_TYPE) {
            continue;
        }
        for property in &view.properties {
            if property.predicate.starts_with("onto:") {
                continue;
            }
            // An unresolved conflict has no usable value but is still a use of the predicate.
            let used = if property.values.is_empty() {
                property.conflict.as_ref().and_then(|c| c.candidates.first()).into_iter().collect::<Vec<_>>()
            } else {
                property.values.iter().collect()
            };
            for value in used {
                let object = match &value.object {
                    Object::Entity(id) => Some(id.clone()),
                    Object::Literal(_) => None,
                };
                values.push((subject.clone(), property.predicate.clone(), object));
            }
        }
        let label = view.text(vocab::LABEL).filter(|l| *l != subject).map(str::to_string);
        if types.iter().any(|t| t == "BusinessConcept") {
            let aliases = view.property(vocab::ALIAS).map(|p| p.values.iter().map(|v| compact::object(&v.object)).collect()).unwrap_or_default();
            concepts.push(Concept { id: subject.clone(), label: label.clone(), aliases });
        }
        if types.iter().any(|t| t == "ModellingStandard" || t == "Decision") {
            standards.push(Standard { id: subject.clone(), label, definition: definition(&view, STANDARD_DEFINITION_CHARS, true) });
        }
        entity_types.insert(subject, types);
    }

    // Types: how many entities have each, with its definition.
    let mut type_counts: BTreeMap<String, usize> = declared_types.keys().map(|t| (t.clone(), 0)).collect();
    let mut untyped = 0;
    for types in entity_types.values() {
        if types.is_empty() {
            untyped += 1;
        }
        for t in types {
            *type_counts.entry(t.clone()).or_default() += 1;
        }
    }
    let mut type_rows: Vec<(usize, String)> = type_counts.into_iter().map(|(t, n)| (n, t)).collect();
    type_rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut type_lines: Vec<String> = type_rows
        .into_iter()
        .map(|(n, t)| match declared_types.get(&t).cloned().flatten() {
            Some(definition) => format!("{t} ({n}): {definition}"),
            None => format!("{t} ({n})"),
        })
        .collect();
    if untyped > 0 {
        type_lines.push(format!("{UNTYPED} ({untyped}): entities with no type recorded"));
    }

    // Predicates: how often each is used and which types it links. Relationships
    // also count towards each end's connections.
    let mut by_predicate: BTreeMap<String, BTreeMap<(String, String), usize>> = BTreeMap::new();
    let mut degree: HashMap<&str, usize> = HashMap::new();
    let mut represented_by: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for (subject, predicate, object) in &values {
        let from = type_label(entity_types.get(subject));
        let to = match object {
            Some(object) => type_label(entity_types.get(object)),
            None => TEXT.to_string(),
        };
        *by_predicate.entry(compact::predicate(predicate).to_string()).or_default().entry((from, to)).or_default() += 1;
        if let Some(object) = object {
            *degree.entry(subject).or_default() += 1;
            *degree.entry(object).or_default() += 1;
            if predicate == "represents" {
                represented_by.entry(object).or_default().insert(subject);
            }
        }
    }
    // `represents: DataAsset → BusinessConcept (12)`, or with several type pairs
    // `grain (5): DataAsset → text (3), BusinessConcept → text (2)`.
    let mut predicate_rows: Vec<(usize, String)> = by_predicate
        .into_iter()
        .map(|(predicate, pairs)| {
            let total: usize = pairs.values().sum();
            let mut pairs: Vec<_> = pairs.into_iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let listed: Vec<String> = pairs.iter().take(MAX_PAIRS).map(|((from, to), n)| format!("{from} → {to} ({n})")).collect();
            let more = if pairs.len() > MAX_PAIRS { format!(", +{} more type pairs", pairs.len() - MAX_PAIRS) } else { String::new() };
            let line = if pairs.len() == 1 { format!("{predicate}: {}", listed[0]) } else { format!("{predicate} ({total}): {}{more}", listed.join(", ")) };
            (total, line)
        })
        .collect();
    // Most used first; ties by name (each line starts with its predicate).
    predicate_rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let predicate_lines: Vec<String> = predicate_rows.into_iter().map(|(_, line)| line).collect();

    // Main concepts: most connected first.
    concepts.sort_by(|a, b| {
        let (da, db) = (degree.get(a.id.as_str()).copied().unwrap_or(0), degree.get(b.id.as_str()).copied().unwrap_or(0));
        db.cmp(&da).then_with(|| a.id.cmp(&b.id))
    });
    let concept_lines: Vec<String> = concepts
        .iter()
        .map(|c| {
            let mut line = c.id.clone();
            if let Some(label) = &c.label {
                line.push_str(&format!(" \"{}\"", shown(label, SHORT_TEXT_CHARS)));
            }
            let links = degree.get(c.id.as_str()).copied().unwrap_or(0);
            line.push_str(&format!(" ({links} {})", if links == 1 { "link" } else { "links" }));
            if !c.aliases.is_empty() {
                let mut aliases: Vec<String> = c.aliases.iter().take(MAX_ALIASES).map(|a| shown(a, SHORT_TEXT_CHARS)).collect();
                if c.aliases.len() > MAX_ALIASES {
                    aliases.push(format!("+{} more", c.aliases.len() - MAX_ALIASES));
                }
                line.push_str(&format!("; also called: {}", aliases.join(", ")));
            }
            if let Some(assets) = represented_by.get(c.id.as_str()) {
                let mut listed: Vec<String> = assets.iter().take(MAX_REPRESENTED_BY).map(|a| a.to_string()).collect();
                if assets.len() > MAX_REPRESENTED_BY {
                    listed.push(format!("+{} more", assets.len() - MAX_REPRESENTED_BY));
                }
                line.push_str(&format!("; represented by: {}", listed.join(", ")));
            }
            line
        })
        .collect();

    let standard_lines: Vec<String> = standards
        .iter()
        .map(|s| {
            let mut line = s.id.clone();
            if let Some(label) = &s.label {
                line.push_str(&format!(" \"{}\"", shown(label, SHORT_TEXT_CHARS)));
            }
            if let Some(definition) = &s.definition {
                line.push_str(&format!(": {definition}"));
            }
            line
        })
        .collect();

    // Fill the budget section by section; health is always included.
    let mut used = serde_json::to_string(&health).map_err(err)?.len();
    let mut fill = |lines: Vec<String>, cap: usize| -> (Vec<String>, usize) {
        let total = lines.len();
        let mut kept = Vec::new();
        for line in lines.into_iter().take(cap) {
            if used + line.len() + 3 > budget {
                break;
            }
            used += line.len() + 3;
            kept.push(line);
        }
        let skipped = total - kept.len();
        (kept, skipped)
    };
    let (types, skipped_types) = fill(type_lines, MAX_TYPES);
    let (predicates, skipped_predicates) = fill(predicate_lines, MAX_PREDICATES);
    let (concepts, skipped_concepts) = fill(concept_lines, MAX_CONCEPTS);
    let (standards, skipped_standards) = fill(standard_lines, MAX_STANDARDS);

    let mut more = Vec::new();
    for (n, what) in [(skipped_types, "types"), (skipped_predicates, "predicates"), (skipped_concepts, "concepts"), (skipped_standards, "standards")] {
        if n > 0 {
            more.push(format!("{n} {what}"));
        }
    }
    let omitted = (!more.is_empty()).then(|| {
        format!("{} not shown (list caps or the {budget}-character budget): raise max_chars, or use ontology_search with entity_type", more.join(", "))
    });
    Ok(OverviewResponse { types, predicates, concepts, standards, health, omitted, notice: super::NOTICE.into() })
}

/// The status of each source as one line.
pub fn source_line(source: &onto_store::SourceState) -> String {
    format!(
        "{}: {} layer, {}, version {}, synced {}, {} records",
        source.source_id,
        snake(&source.layer),
        source.kind,
        source.version.as_deref().unwrap_or("none"),
        source.synced_at,
        source.record_count
    )
}
