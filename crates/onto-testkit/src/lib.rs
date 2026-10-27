//! Test kit for agent/human interaction with the ontology MCP server.
//!
//! Three tiers share one [`Session`] and [`Transcript`] format:
//! 1. [`scenario`]: scripted YAML scenarios, deterministic, every PR.
//! 2. [`agent`] (feature `llm`): a real model behind an OpenAI-compatible API
//!    (a small local model in CI) with a scripted human, or a second model
//!    playing the person ([`human`]); assertions are invariants.
//! 3. Recorded transcripts from either tier can be replayed with [`replay`].

#[cfg(feature = "llm")]
pub mod agent;
#[cfg(feature = "llm")]
pub mod claude_code;
#[cfg(feature = "llm")]
pub mod eval;
#[cfg(feature = "llm")]
pub mod fake_model;
pub mod fixture;
#[cfg(feature = "llm")]
pub mod human;
pub mod invariants;
pub mod scenario;
pub mod session;
pub mod transcript;

pub use fixture::Fixture;
pub use session::Session;
pub use transcript::{Event, Transcript, replay};
