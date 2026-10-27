//! Reserved predicates and types understood by the core.

pub const TYPE: &str = "onto:type";
pub const LABEL: &str = "onto:label";
pub const DEFINITION: &str = "onto:definition";
pub const ALIAS: &str = "onto:alias";
pub const SUBTYPE_OF: &str = "onto:subtype_of";
pub const CARDINALITY: &str = "onto:cardinality";
pub const RANGE: &str = "onto:range";

pub const PREDICATE_TYPE: &str = "onto:Predicate";
pub const TYPE_TYPE: &str = "onto:Type";

/// Predicates whose literal values are indexed for search.
pub const SEARCHABLE: &[&str] = &[LABEL, DEFINITION, ALIAS];
