//! An ontology workspace: the `onto.toml` in a repository, the sources it
//! configures, the local store they sync into, and the proposal review flow.

pub mod command;
pub mod compact;
pub mod config;
pub mod digest;
pub mod git;
pub mod local;
pub mod lock;
pub mod pack;
pub mod proposals;
pub mod workspace;

pub use config::{Config, SourceConfig, SourceKind};
pub use workspace::{SyncOptions, SyncOutcome, SyncReport, Workspace, WorkspaceError, now_rfc3339};
