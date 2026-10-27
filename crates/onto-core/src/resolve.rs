//! Merge records from several sources into one view per entity.
//!
//! Rules, carried over from dbt-skills' catalog resolver:
//! - Agreement is detected by identical claims (same content id), never by
//!   matching names alone.
//! - Disagreement on a single-valued predicate is a [`Conflict`]. Only the
//!   configured source priority may select a winner; otherwise it stays
//!   unresolved and every candidate is returned with its provenance.
//! - `Proposed` claims are never used as values. They are reported separately
//!   as pending so the agent can see open proposals.

use std::collections::{BTreeMap, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Authority, Claim, Object, Provenance, Record, Validity};
use crate::vocab;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    One,
    Many,
}

/// Predicate cardinalities declared through `onto:cardinality` claims.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    declared: HashMap<String, Cardinality>,
}

impl Schema {
    /// Cardinality declarations resolved like any other value: agreement is
    /// used as is; disagreement goes to the configured source priority; with no
    /// winner the predicate is single-valued, so conflicts stay visible instead
    /// of being hidden as multiple values. Never depends on record order.
    pub fn resolve<'a>(records: impl IntoIterator<Item = &'a Record>, resolver: &Resolver) -> Self {
        let mut by_predicate: BTreeMap<&str, Vec<(usize, Cardinality)>> = BTreeMap::new();
        for record in records {
            if record.claim.predicate != vocab::CARDINALITY || record.provenance.authority == Authority::Proposed {
                continue;
            }
            let cardinality = match record.claim.object.as_text() {
                Some("one") => Cardinality::One,
                Some("many") => Cardinality::Many,
                _ => continue,
            };
            by_predicate.entry(record.claim.subject.as_str()).or_default().push((resolver.rank(&record.provenance.source_id), cardinality));
        }
        let declared = by_predicate
            .into_iter()
            .map(|(predicate, declarations)| {
                let first = declarations[0].1;
                let cardinality = if declarations.iter().all(|(_, c)| *c == first) {
                    first
                } else {
                    let best = declarations.iter().map(|(rank, _)| *rank).min().unwrap_or(usize::MAX);
                    let mut winners = declarations.iter().filter(|(rank, _)| *rank == best).map(|(_, c)| *c);
                    let winner = winners.next().filter(|c| winners.all(|other| other == *c));
                    match (resolver.mode, winner) {
                        (ResolutionMode::Priority, Some(c)) if best < resolver.priority.len() => c,
                        _ => Cardinality::One,
                    }
                };
                (predicate.to_string(), cardinality)
            })
            .collect();
        Self { declared }
    }

    /// Declared cardinality, then built-in vocabulary, then a default:
    /// literal-valued predicates are single-valued, relations are multi-valued.
    pub fn cardinality(&self, predicate: &str, object: &Object) -> Cardinality {
        if let Some(c) = self.declared.get(predicate) {
            return *c;
        }
        match predicate {
            vocab::TYPE | vocab::ALIAS | vocab::SUBTYPE_OF => Cardinality::Many,
            vocab::LABEL | vocab::DEFINITION | vocab::CARDINALITY | vocab::RANGE => Cardinality::One,
            _ if object.entity().is_some() => Cardinality::Many,
            _ => Cardinality::One,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMode {
    /// The highest-priority configured source wins a conflict; the conflict is still reported.
    #[default]
    Priority,
    /// Never select a winner; every conflict stays unresolved.
    Preserve,
}

/// One distinct value with every source that asserted it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Value {
    pub object: Object,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub qualifiers: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Validity::is_open")]
    pub valid: Validity,
    pub sources: Vec<Provenance>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConflictStatus {
    ResolvedByPriority { selected_source: String },
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Conflict {
    #[serde(flatten)]
    pub status: ConflictStatus,
    pub candidates: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Property {
    pub predicate: String,
    pub cardinality: Cardinality,
    /// Usable values. Empty when a conflict is unresolved.
    pub values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<Conflict>,
    /// Open proposals for this predicate; not facts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EntityView {
    pub id: String,
    pub properties: Vec<Property>,
}

impl EntityView {
    pub fn property(&self, predicate: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.predicate == predicate)
    }

    pub fn text(&self, predicate: &str) -> Option<&str> {
        self.property(predicate)?.values.first()?.object.as_text()
    }

    pub fn entities(&self, predicate: &str) -> Vec<&str> {
        self.property(predicate).map(|p| p.values.iter().filter_map(|v| v.object.entity()).collect()).unwrap_or_default()
    }

    pub fn has_unresolved_conflict(&self) -> bool {
        self.properties.iter().any(|p| matches!(p.conflict, Some(Conflict { status: ConflictStatus::Unresolved, .. })))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// Source ids, highest priority first. Unlisted sources rank below all listed ones.
    pub priority: Vec<String>,
    pub mode: ResolutionMode,
    /// Only claims valid at this instant are considered; `None` ignores validity.
    pub as_of: Option<String>,
}

impl Resolver {
    fn rank(&self, source_id: &str) -> usize {
        self.priority.iter().position(|s| s == source_id).unwrap_or(self.priority.len())
    }

    /// Resolve `id` from records whose subject is `id`.
    pub fn resolve_entity(&self, id: &str, records: &[Record], schema: &Schema) -> EntityView {
        let mut by_predicate: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
        for record in records.iter().filter(|r| r.claim.subject == id) {
            if self.as_of.as_deref().is_some_and(|t| !record.claim.valid.holds_at(t)) {
                continue;
            }
            by_predicate.entry(record.claim.predicate.as_str()).or_default().push(record);
        }
        let properties = by_predicate.into_iter().map(|(predicate, records)| self.resolve_property(predicate, &records, schema)).collect();
        EntityView { id: id.to_string(), properties }
    }

    fn resolve_property(&self, predicate: &str, records: &[&Record], schema: &Schema) -> Property {
        let (pending, asserted): (Vec<&Record>, Vec<&Record>) = records.iter().partition(|r| r.provenance.authority == Authority::Proposed);
        let candidates = group(&asserted);
        let cardinality = candidates.first().or(group(&pending).first()).map(|v| schema.cardinality(predicate, &v.object)).unwrap_or(Cardinality::Many);
        let mut property = Property { predicate: predicate.to_string(), cardinality, values: Vec::new(), conflict: None, pending: group(&pending) };

        if cardinality == Cardinality::Many || candidates.len() <= 1 {
            property.values = candidates;
            return property;
        }

        let selected = match self.mode {
            ResolutionMode::Preserve => None,
            ResolutionMode::Priority => self.select(&candidates),
        };
        property.conflict = Some(match selected {
            Some(index) => {
                let winner = candidates[index].clone();
                let selected_source = winner.sources.iter().min_by_key(|p| self.rank(&p.source_id)).expect("value has a source").source_id.clone();
                property.values = vec![winner];
                Conflict { status: ConflictStatus::ResolvedByPriority { selected_source }, candidates }
            }
            None => Conflict { status: ConflictStatus::Unresolved, candidates },
        });
        property
    }

    /// Index of the single candidate backed by the best-ranked listed source.
    fn select(&self, candidates: &[Value]) -> Option<usize> {
        let ranks: Vec<usize> = candidates.iter().map(|v| v.sources.iter().map(|p| self.rank(&p.source_id)).min().unwrap_or(usize::MAX)).collect();
        let best = *ranks.iter().min()?;
        if best >= self.priority.len() || ranks.iter().filter(|r| **r == best).count() > 1 {
            return None;
        }
        ranks.iter().position(|r| *r == best)
    }
}

/// Collapse identical claims into one value carrying all their sources.
fn group(records: &[&Record]) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut values: HashMap<String, Value> = HashMap::new();
    for record in records {
        let Claim { object, qualifiers, valid, .. } = &record.claim;
        let key = record.claim.id();
        values
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                Value { object: object.clone(), qualifiers: qualifiers.clone(), valid: valid.clone(), sources: Vec::new() }
            })
            .sources
            .push(record.provenance.clone());
    }
    order.into_iter().map(|k| values.remove(&k).expect("grouped value")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Layer, Provenance};

    fn rec(claim: Claim, source: &str, authority: Authority) -> Record {
        Record { claim, provenance: Provenance::new(source, Layer::Repository, authority) }
    }

    fn def(source: &str, text: &str) -> Record {
        rec(Claim::literal("concept:customer", vocab::DEFINITION, text), source, Authority::Declared)
    }

    #[test]
    fn agreement_merges_sources() {
        let records = [def("central", "A buyer"), def("repo", "A buyer")];
        let view = Resolver::default().resolve_entity("concept:customer", &records, &Schema::default());
        let p = view.property(vocab::DEFINITION).unwrap();
        assert!(p.conflict.is_none());
        assert_eq!(p.values[0].sources.len(), 2);
    }

    #[test]
    fn priority_selects_but_keeps_conflict() {
        let records = [def("central", "A buyer"), def("repo", "An account holder")];
        let resolver = Resolver { priority: vec!["repo".into(), "central".into()], ..Default::default() };
        let view = resolver.resolve_entity("concept:customer", &records, &Schema::default());
        let p = view.property(vocab::DEFINITION).unwrap();
        assert_eq!(view.text(vocab::DEFINITION), Some("An account holder"));
        let conflict = p.conflict.as_ref().unwrap();
        assert_eq!(conflict.status, ConflictStatus::ResolvedByPriority { selected_source: "repo".into() });
        assert_eq!(conflict.candidates.len(), 2);
    }

    #[test]
    fn unlisted_or_preserve_leaves_conflict_unresolved() {
        let records = [def("central", "A buyer"), def("repo", "An account holder")];
        let unlisted = Resolver::default().resolve_entity("concept:customer", &records, &Schema::default());
        assert!(unlisted.has_unresolved_conflict());
        assert!(unlisted.text(vocab::DEFINITION).is_none());

        let preserve = Resolver { priority: vec!["repo".into()], mode: ResolutionMode::Preserve, as_of: None };
        assert!(preserve.resolve_entity("concept:customer", &records, &Schema::default()).has_unresolved_conflict());
    }

    #[test]
    fn same_source_disagreeing_with_itself_is_unresolved() {
        let records = [def("repo", "A buyer"), def("repo", "An account holder")];
        let resolver = Resolver { priority: vec!["repo".into()], ..Default::default() };
        assert!(resolver.resolve_entity("concept:customer", &records, &Schema::default()).has_unresolved_conflict());
    }

    #[test]
    fn proposals_are_pending_not_values() {
        let records = [def("central", "A buyer"), rec(Claim::literal("concept:customer", vocab::DEFINITION, "Someone"), "proposal", Authority::Proposed)];
        let view = Resolver::default().resolve_entity("concept:customer", &records, &Schema::default());
        let p = view.property(vocab::DEFINITION).unwrap();
        assert_eq!(p.values.len(), 1);
        assert!(p.conflict.is_none());
        assert_eq!(p.pending.len(), 1);
    }

    #[test]
    fn relations_are_multi_valued_and_declared_cardinality_wins() {
        let records = [
            rec(Claim::relation("concept:customer", "places", "concept:order"), "central", Authority::Declared),
            rec(Claim::relation("concept:customer", "places", "concept:return"), "repo", Authority::Declared),
        ];
        let view = Resolver::default().resolve_entity("concept:customer", &records, &Schema::default());
        assert_eq!(view.entities("places").len(), 2);

        let schema_records = [rec(Claim::literal("places", vocab::CARDINALITY, "one"), "central", Authority::Declared)];
        let schema = Schema::resolve(&schema_records, &Resolver::default());
        assert!(Resolver::default().resolve_entity("concept:customer", &records, &schema).has_unresolved_conflict());
    }

    #[test]
    fn competing_cardinality_declarations_follow_priority_not_order() {
        let decl = |source: &str, value: &str| rec(Claim::literal("places", vocab::CARDINALITY, value), source, Authority::Declared);
        let priority = Resolver { priority: vec!["repo".into(), "central".into()], ..Default::default() };
        let object = Object::Entity("x".into());
        for records in [[decl("central", "many"), decl("repo", "one")], [decl("repo", "one"), decl("central", "many")]] {
            assert_eq!(Schema::resolve(&records, &priority).cardinality("places", &object), Cardinality::One);
        }
        // Without a priority winner, the conservative answer keeps conflicts visible.
        let preserve = Resolver { mode: ResolutionMode::Preserve, ..priority.clone() };
        assert_eq!(Schema::resolve(&[decl("repo", "many"), decl("central", "one")], &preserve).cardinality("places", &object), Cardinality::One);
        assert_eq!(Schema::resolve(&[decl("repo", "many"), decl("central", "many")], &preserve).cardinality("places", &object), Cardinality::Many);
    }

    #[test]
    fn as_of_filters_historical_claims() {
        let mut old = Claim::literal("concept:customer", "grain", "one row per account");
        old.valid.until = Some("2024-01-01".into());
        let mut new = Claim::literal("concept:customer", "grain", "one row per person");
        new.valid.from = Some("2024-01-01".into());
        let records = [rec(old, "repo", Authority::Reviewed), rec(new, "repo", Authority::Reviewed)];
        let resolver = Resolver { as_of: Some("2025-06-01".into()), ..Default::default() };
        let view = resolver.resolve_entity("concept:customer", &records, &Schema::default());
        assert_eq!(view.text("grain"), Some("one row per person"));
        assert!(!view.has_unresolved_conflict());
    }
}
