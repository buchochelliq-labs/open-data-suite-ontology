//! `ontology_path`: how two entities are connected (#14).
//!
//! A breadth-first search over relationships in both directions, up to a hop
//! limit, returning the shortest paths. Every step is a relationship as a
//! source states it (subject, predicate, object), with its qualifiers, source
//! and authority, so a step walked backwards still reads the right way round.
//!
//! Relationships hold now or not at all: one that has ended or not begun is
//! not a step. Known-issue `affects` links, types and wiki articles are not
//! steps: an issue or an article mentioning two things does not connect them.
//! Steps from OpenDataSuite are dbt structure (columns and keys), labelled as
//! such: a path is never lineage.

use std::collections::{BTreeSet, HashMap, VecDeque};

use onto_core::{EntityView, Object, Value, vocab};
use onto_store::{Edge, Store};
use onto_workspace::{Workspace, compact, issues};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_HOPS: u8 = 4;
pub const MAX_HOPS: u8 = 6;
/// Shortest paths returned at most; more of the same length are counted.
const MAX_PATHS: usize = 3;
/// Entities expanded at most, so a hub (a model with hundreds of columns)
/// cannot make one call unbounded. Reaching it is reported.
const MAX_EXPANDED: usize = 5_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PathRequest {
    /// Where the path starts: a concept id or dbt node id.
    pub from: String,
    /// Where it ends.
    pub to: String,
    /// Longest path in relationships (default 4, at most 6).
    #[serde(default)]
    pub max_hops: Option<u8>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Path {
    pub hops: usize,
    /// One relationship per step, as its source states it:
    /// `subject predicate → object (qualifiers) [source authority]`.
    pub steps: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PathResponse {
    pub found: bool,
    /// Shortest paths first, at most three.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<Path>,
    /// Why nothing was found, or what was left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Ids that are not in the graph, with the closest matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<String>,
    pub notice: String,
}

/// The relationships an entity asserts now, as (predicate, object, qualifiers).
type Current = BTreeSet<(String, String, String)>;

fn current(view: &EntityView) -> Current {
    let mut out = BTreeSet::new();
    for property in &view.properties {
        let values = property.values.iter().chain(property.conflict.iter().flat_map(|c| c.candidates.iter()));
        for value in values {
            if let Object::Entity(target) = &value.object {
                out.insert((property.predicate.clone(), target.clone(), serde_json::to_string(&value.qualifiers).unwrap_or_default()));
            }
        }
    }
    out
}

/// Find the shortest paths from `req.from` to `req.to`.
pub fn find(
    workspace: &Workspace,
    store: &Store,
    req: &PathRequest,
    resolve: &dyn Fn(&str) -> Result<EntityView, String>,
    notice: &str,
) -> Result<PathResponse, String> {
    let err = |e: onto_store::StoreError| e.to_string();
    let max_hops = req.max_hops.unwrap_or(DEFAULT_HOPS).clamp(1, MAX_HOPS) as usize;
    let mut not_found = Vec::new();
    for id in [&req.from, &req.to] {
        if !store.entity_exists(id).map_err(err)? {
            let close: Vec<String> = store.search(id, 3).map_err(err)?.into_iter().map(|h| h.id).collect();
            not_found.push(if close.is_empty() { id.clone() } else { format!("{id} (closest: {})", close.join(", ")) });
        }
    }
    let response = |found, paths, note: Option<String>, not_found| PathResponse { found, paths, note, not_found, notice: notice.to_string() };
    if !not_found.is_empty() {
        return Ok(response(false, Vec::new(), None, not_found));
    }
    if req.from == req.to {
        return Ok(response(true, vec![Path { hops: 0, steps: Vec::new() }], Some("from and to are the same entity".into()), Vec::new()));
    }

    let skipped: BTreeSet<String> = workspace.wiki_sources();
    let mut asserted: HashMap<String, Current> = HashMap::new();
    let mut holds = |edge: &Edge| -> Result<bool, String> {
        if !asserted.contains_key(&edge.subject) {
            asserted.insert(edge.subject.clone(), current(&resolve(&edge.subject)?));
        }
        let key = (edge.predicate.clone(), edge.object.clone(), serde_json::to_string(&edge.qualifiers).unwrap_or_default());
        Ok(asserted[&edge.subject].contains(&key))
    };

    // Breadth-first by layers, keeping every edge that reaches a node at its
    // shortest distance, so all shortest paths can be rebuilt.
    let mut depth: HashMap<String, usize> = HashMap::from([(req.from.clone(), 0)]);
    let mut parents: HashMap<String, Vec<(String, Edge)>> = HashMap::new();
    let mut queue = VecDeque::from([req.from.clone()]);
    let mut expanded = 0;
    let mut truncated = false;
    while let Some(node) = queue.pop_front() {
        let d = depth[&node];
        if d >= max_hops || depth.get(&req.to).is_some_and(|&t| d >= t) {
            continue;
        }
        if expanded == MAX_EXPANDED {
            truncated = true;
            break;
        }
        expanded += 1;
        for edge in store.edges_of(&node).map_err(err)? {
            if edge.predicate == vocab::TYPE
                || edge.predicate == issues::AFFECTS
                || edge.sources.iter().all(|p| skipped.contains(&p.source_id))
                || edge.subject == edge.object
            {
                continue;
            }
            let next = if edge.subject == node { edge.object.clone() } else { edge.subject.clone() };
            match depth.get(&next) {
                Some(&seen) if seen < d + 1 => continue,
                Some(_) => {}
                None => {
                    if !holds(&edge)? {
                        continue;
                    }
                    depth.insert(next.clone(), d + 1);
                    queue.push_back(next.clone());
                    parents.entry(next).or_default().push((node.clone(), edge));
                    continue;
                }
            }
            if holds(&edge)? {
                parents.entry(next).or_default().push((node.clone(), edge));
            }
        }
    }

    let Some(&hops) = depth.get(&req.to) else {
        let note = if truncated {
            format!("no path found before the search reached its limit of {MAX_EXPANDED} entities; try entities closer together, or ontology_context")
        } else {
            format!(
                "no path within {max_hops} relationships (the team's facts and dbt structure, as they hold now); raise max_hops up to {MAX_HOPS}, or the two may not be connected"
            )
        };
        return Ok(response(false, Vec::new(), Some(note), Vec::new()));
    };

    // Rebuild paths backwards from `to`, in a stable order.
    let mut paths: Vec<Vec<Edge>> = Vec::new();
    let mut total = 0usize;
    let mut stack: Vec<(String, Vec<Edge>)> = vec![(req.to.clone(), Vec::new())];
    while let Some((node, suffix)) = stack.pop() {
        if node == req.from {
            total += 1;
            if paths.len() < MAX_PATHS {
                paths.push(suffix.into_iter().rev().collect());
            }
            continue;
        }
        let mut options: Vec<&(String, Edge)> = parents.get(&node).map(|p| p.iter().collect()).unwrap_or_default();
        options.sort_by(|a, b| (&b.0, &b.1.predicate).cmp(&(&a.0, &a.1.predicate)));
        options.dedup_by(|a, b| a.0 == b.0 && a.1.predicate == b.1.predicate && a.1.subject == b.1.subject && a.1.qualifiers == b.1.qualifiers);
        for (parent, edge) in options {
            let mut next = suffix.clone();
            next.push(edge.clone());
            stack.push((parent.clone(), next));
        }
        // Counting every path of a dense graph is not worth the time.
        if total > 50 {
            break;
        }
    }

    let ods: BTreeSet<String> =
        workspace.config.sources.iter().filter(|s| matches!(s.kind, onto_workspace::SourceKind::Ods { .. })).map(|s| s.id.clone()).collect();
    let mut uses_ods = false;
    let paths: Vec<Path> = paths
        .into_iter()
        .map(|edges| Path {
            hops,
            steps: edges
                .iter()
                .map(|edge| {
                    let dbt = edge.sources.iter().all(|p| ods.contains(&p.source_id));
                    uses_ods |= dbt;
                    let value = Value {
                        object: Object::Entity(edge.object.clone()),
                        qualifiers: edge.qualifiers.clone(),
                        valid: Default::default(),
                        sources: edge.sources.clone(),
                    };
                    format!("{} {}{}", edge.subject, compact::line(&edge.predicate, &value), if dbt { " (dbt structure)" } else { "" })
                })
                .collect(),
        })
        .collect();
    let mut notes = Vec::new();
    if total > paths.len() {
        notes.push(format!("{} more paths of the same length", total - paths.len()));
    }
    if uses_ods {
        notes.push("steps marked dbt structure come from OpenDataSuite (columns and keys); they are not lineage, ask ods_lineage for that".to_string());
    }
    Ok(response(true, paths, (!notes.is_empty()).then(|| notes.join("; ")), Vec::new()))
}
