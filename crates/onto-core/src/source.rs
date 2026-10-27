use serde::{Deserialize, Serialize};

use crate::model::{Layer, Record};

/// Static facts about a configured source.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub kind: String,
    pub layer: Layer,
}

/// One page of a source snapshot. The store only replaces a source's records
/// once every page has been received, so a failed sync never leaves half a
/// generation behind.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub records: Vec<Record>,
    pub next_cursor: Option<String>,
    /// Version of the snapshot this page belongs to (commit, ETag, revision).
    pub version: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("source `{source_id}` is unavailable: {message}")]
    Unavailable { source_id: String, message: String },
    #[error("source `{source_id}` denied access: {message}")]
    Denied { source_id: String, message: String },
    #[error("source `{source_id}` returned invalid data: {message}")]
    Invalid { source_id: String, message: String },
}

/// The only thing the rest of the system knows about where facts come from.
///
/// Git packs, local files, command adapters, catalogs and a future hosted
/// service all implement this. Implementations must stamp every record with
/// their own `source_id` and `layer`; the sync layer re-checks this so a source
/// cannot impersonate another.
pub trait Source: Send + Sync {
    fn info(&self) -> SourceInfo;

    /// Fetch the page after `cursor` (`None` starts a full snapshot).
    fn fetch(&self, cursor: Option<&str>) -> Result<Page, SourceError>;
}
