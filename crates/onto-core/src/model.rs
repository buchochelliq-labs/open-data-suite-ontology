use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::canonical_digest;

/// Content-derived claim identifier (hex SHA-256 of the canonical claim).
pub type ClaimId = String;

/// The object of a claim: a literal value or a reference to another entity.
///
/// Serialized as a single-key map, `{"literal": value}` or `{"entity": "id"}`,
/// in every format (YAML libraries would otherwise use `!tags`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(into = "ObjectRepr", try_from = "ObjectRepr")]
pub enum Object {
    Literal(serde_json::Value),
    Entity(String),
}

/// Wire form of [`Object`]: exactly one of the two keys.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectRepr {
    /// A literal value (string, number, boolean, list or map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<serde_json::Value>,
    /// The id of another entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
}

impl From<Object> for ObjectRepr {
    fn from(object: Object) -> Self {
        match object {
            Object::Literal(v) => ObjectRepr { literal: Some(v), entity: None },
            Object::Entity(id) => ObjectRepr { literal: None, entity: Some(id) },
        }
    }
}

impl TryFrom<ObjectRepr> for Object {
    type Error = String;

    fn try_from(repr: ObjectRepr) -> Result<Self, Self::Error> {
        match (repr.literal, repr.entity) {
            (Some(v), None) => Ok(Object::Literal(v)),
            (None, Some(id)) if !id.trim().is_empty() => Ok(Object::Entity(id)),
            (None, Some(_)) => Err("entity id must not be empty".into()),
            _ => Err("object needs exactly one of `literal` or `entity`".into()),
        }
    }
}

impl Object {
    pub fn entity(&self) -> Option<&str> {
        match self {
            Object::Entity(id) => Some(id),
            Object::Literal(_) => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Object::Literal(serde_json::Value::String(s)) => Some(s),
            _ => None,
        }
    }
}

/// When a claim holds in the described world (as opposed to when we learned it,
/// which lives in [`Provenance`]). Together they make claims bi-temporal.
/// Bounds are ISO-8601 strings; `until` is exclusive.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Validity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

impl Validity {
    pub fn is_open(&self) -> bool {
        self.from.is_none() && self.until.is_none()
    }

    /// Lexicographic comparison; callers must use one ISO-8601 format consistently.
    pub fn holds_at(&self, instant: &str) -> bool {
        self.from.as_deref().is_none_or(|from| from <= instant) && self.until.as_deref().is_none_or(|until| instant < until)
    }
}

/// One assertion. Qualifiers describe the statement itself (for example the
/// cardinality of a relationship) rather than the subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claim {
    pub subject: String,
    pub predicate: String,
    pub object: Object,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub qualifiers: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Validity::is_open")]
    pub valid: Validity,
}

impl Claim {
    pub fn new(subject: impl Into<String>, predicate: impl Into<String>, object: Object) -> Self {
        Self { subject: subject.into(), predicate: predicate.into(), object, qualifiers: BTreeMap::new(), valid: Validity::default() }
    }

    pub fn literal(subject: impl Into<String>, predicate: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new(subject, predicate, Object::Literal(value.into()))
    }

    pub fn relation(subject: impl Into<String>, predicate: impl Into<String>, target: impl Into<String>) -> Self {
        Self::new(subject, predicate, Object::Entity(target.into()))
    }

    /// Identical statements from different sources share an id, which is how
    /// agreement between sources is detected.
    pub fn id(&self) -> ClaimId {
        canonical_digest(self)
    }
}

/// Where a claim sits in the knowledge stack. Layers say where the claim lives,
/// not how much to trust it; that is [`Authority`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// Shared ontology consumed from a central location (git pack today, a hosted service later).
    Central,
    /// Facts committed to, or derived from, the consuming repository.
    Repository,
    /// Personal, uncommitted overlay on one machine.
    Local,
}

/// How a claim came to be asserted. Kept separate so that inferences are never
/// silently presented as agreed facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Accepted by a human through an explicit review step.
    Reviewed,
    /// Asserted by a system that owns the fact (catalog, central pack, contract).
    Declared,
    /// Derived deterministically from an artifact (manifest, schema, file).
    Observed,
    /// Heuristic or model-produced hypothesis.
    Inferred,
    /// Awaiting review; never used to answer what is true.
    Proposed,
}

/// Who said a claim, where, and when we learned it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    pub source_id: String,
    pub layer: Layer,
    pub authority: Authority,
    /// Native locator inside the source, e.g. `concepts.yaml#concept:customer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// Version of the source snapshot (commit, ETag, catalog revision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributed_to: Option<String>,
    /// Claims this one was derived from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<ClaimId>,
}

impl Provenance {
    pub fn new(source_id: impl Into<String>, layer: Layer, authority: Authority) -> Self {
        Self {
            source_id: source_id.into(),
            layer,
            authority,
            locator: None,
            source_version: None,
            retrieved_at: None,
            rationale: None,
            attributed_to: None,
            derived_from: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Record {
    pub claim: Claim,
    pub provenance: Provenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_claims_share_an_id() {
        let a = Claim::literal("concept:customer", "onto:label", "Customer");
        let b = Claim::literal("concept:customer", "onto:label", "Customer");
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), Claim::literal("concept:customer", "onto:label", "Client").id());
    }

    #[test]
    fn object_wire_form_is_a_single_key_map() {
        let json = serde_json::to_value(Object::Entity("concept:order".into())).unwrap();
        assert_eq!(json, serde_json::json!({"entity": "concept:order"}));
        let back: Object = serde_json::from_value(serde_json::json!({"literal": 3})).unwrap();
        assert_eq!(back, Object::Literal(3.into()));
        assert!(serde_json::from_value::<Object>(serde_json::json!({"literal": 1, "entity": "x"})).is_err());
        assert!(serde_json::from_value::<Object>(serde_json::json!({})).is_err());
    }

    #[test]
    fn validity_bounds_are_half_open() {
        let v = Validity { from: Some("2024-01-01".into()), until: Some("2025-01-01".into()) };
        assert!(v.holds_at("2024-06-01"));
        assert!(!v.holds_at("2025-01-01"));
        assert!(!v.holds_at("2023-12-31"));
    }
}
