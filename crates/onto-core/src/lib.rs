//! Core model for a layered, provenance-preserving ontology.
//!
//! Everything is a [`Claim`]: a subject/predicate/object statement. A claim only
//! becomes usable context together with its [`Provenance`] (which source said it,
//! in which layer, with what authority), forming a [`Record`]. Several sources can
//! make the same or competing claims; the [`resolve`] module merges them without
//! hiding disagreement.

pub mod digest;
pub mod model;
pub mod resolve;
pub mod source;
pub mod vocab;

pub use model::{Authority, Claim, ClaimId, Layer, Object, Provenance, Record, Validity};
pub use resolve::{Cardinality, Conflict, ConflictStatus, EntityView, Property, ResolutionMode, Resolver, Schema, Value};
pub use source::{Page, Source, SourceError, SourceInfo};
