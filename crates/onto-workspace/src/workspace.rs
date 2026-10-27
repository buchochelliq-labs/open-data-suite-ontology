use std::path::{Path, PathBuf};

use onto_core::{Layer, Record, ResolutionMode, Resolver, Schema, Source, vocab};
use onto_store::{SourceState, Store, StoreError};
use serde::Serialize;

use crate::command::CommandSource;
use crate::config::{CONFIG_FILE, Config, SourceConfig, SourceKind};
use crate::git::GitSource;
use crate::local::{LocalConfig, Trust, TrustedCommand};
use crate::lock::{Lock, LockedSource};
use crate::pack::PackSource;

/// Directory, relative to the workspace root, for machine-local state.
pub const STATE_DIR: &str = ".onto";
const MAX_PAGES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).expect("formattable time")
}

pub struct Workspace {
    pub root: PathBuf,
    /// Shared configuration with this machine's `.onto/local.toml` applied.
    pub config: Config,
    /// Sources disabled on this machine. Their lock entries are kept.
    pub disabled: Vec<String>,
    /// Sources whose command was set in `.onto/local.toml`.
    own_commands: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SyncOptions {
    /// Use `onto.lock` commits and fail if a git source is not locked (CI mode).
    pub locked: bool,
    /// Only sync these sources; all others keep their current generation.
    pub only: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SyncOutcome {
    Synced {
        version: Option<String>,
        records: usize,
    },
    /// The previous generation is kept (stale-on-error).
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct SyncReport {
    pub source_id: String,
    #[serde(flatten)]
    pub outcome: SyncOutcome,
}

impl Workspace {
    pub fn load(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let path = root.join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| WorkspaceError::Config(format!("cannot read {}: {e}", path.display())))?;
        let shared: Config = toml::from_str(&text).map_err(|e| WorkspaceError::Config(format!("{}: {e}", path.display())))?;
        Self { root: root.clone(), config: shared.clone(), disabled: Vec::new(), own_commands: Vec::new() }.validate()?;
        let mut config = shared;
        let applied = LocalConfig::read(&root)?.apply(&mut config)?;
        let workspace = Self { root, config, disabled: applied.disabled, own_commands: applied.own_commands };
        workspace.validate()?;
        Ok(workspace)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for source in &self.config.sources {
            if source.id.is_empty() || !source.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(WorkspaceError::Config(format!("source id `{}` must be non-empty ASCII letters, digits, - or _", source.id)));
            }
            if !seen.insert(&source.id) {
                return Err(WorkspaceError::Config(format!("duplicate source id `{}`", source.id)));
            }
        }
        for id in &self.config.resolution.priority {
            if !seen.contains(id) {
                return Err(WorkspaceError::Config(format!("resolution priority names unknown source `{id}`")));
            }
        }
        Ok(())
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    pub fn open_store(&self) -> Result<Store> {
        std::fs::create_dir_all(self.state_dir())?;
        Ok(Store::open(&self.state_dir().join("store.db"))?)
    }

    pub fn source_config(&self, id: &str) -> Option<&SourceConfig> {
        self.config.sources.iter().find(|s| s.id == id)
    }

    pub fn resolver(&self, as_of: Option<String>) -> Resolver {
        Resolver { priority: self.config.resolution.priority.clone(), mode: self.config.resolution.mode, as_of }
    }

    pub fn resolution_mode(&self) -> ResolutionMode {
        self.config.resolution.mode
    }

    pub fn schema(&self, store: &Store) -> Result<Schema> {
        let records: Vec<Record> = store.records_with_predicate(vocab::CARDINALITY)?;
        Ok(Schema::resolve(&records, &self.resolver(None)))
    }

    /// Directory of a local pack source.
    pub fn pack_dir(&self, source: &SourceConfig) -> Option<PathBuf> {
        match &source.kind {
            SourceKind::Pack { path } => Some(self.root.join(path)),
            _ => None,
        }
    }

    /// The command a source would run that needs this person's trust, if any.
    /// The built-in `ods` default and commands set in `.onto/local.toml` need none.
    pub fn command_needing_trust(&self, source: &SourceConfig) -> Option<Vec<String>> {
        if self.own_commands.contains(&source.id) {
            return None;
        }
        match &source.kind {
            SourceKind::Command { command, .. } => Some(command.clone()),
            SourceKind::Ods { command, .. } if command.as_slice() != ["ods"] => Some(command.clone()),
            _ => None,
        }
    }

    /// Record that this person allows `source_id` to run its current command.
    pub fn trust(&self, source_id: &str) -> Result<Option<Vec<String>>> {
        let source = self.source_config(source_id).ok_or_else(|| WorkspaceError::Config(format!("unknown source `{source_id}`")))?;
        let Some(command) = self.command_needing_trust(source) else {
            return Ok(None);
        };
        let mut trust = Trust::read(&self.root)?;
        trust.entries.retain(|t| t.source != source_id);
        trust.entries.push(TrustedCommand { source: source_id.to_string(), command: command.clone(), trusted_at: now_rfc3339() });
        trust.write(&self.root)?;
        Ok(Some(command))
    }

    fn check_trust(&self, source: &SourceConfig) -> Result<()> {
        if let Some(command) = self.command_needing_trust(source)
            && !Trust::read(&self.root)?.allows(&source.id, &command)
        {
            return Err(WorkspaceError::Config(format!(
                "source `{}` would run {:?} from onto.toml, which this machine has not trusted (or it changed since). Review it, then run `onto trust {}`",
                source.id, command, source.id
            )));
        }
        Ok(())
    }

    fn build(&self, source: &SourceConfig, lock: Option<&Lock>, locked: bool) -> Result<Box<dyn Source>> {
        Ok(match &source.kind {
            SourceKind::Pack { path } => Box::new(PackSource::new(&source.id, source.layer, self.root.join(path))),
            SourceKind::Git { url, rev, path } => {
                let pinned = lock.and_then(|l| l.get(&source.id, url)).map(|l| l.commit.clone());
                if locked && pinned.is_none() {
                    return Err(WorkspaceError::Config(format!("source `{}` is not in onto.lock; run `onto sync` without --locked", source.id)));
                }
                Box::new(GitSource {
                    id: source.id.clone(),
                    layer: source.layer,
                    url: url.clone(),
                    rev: rev.clone(),
                    path: path.clone(),
                    checkout: self.state_dir().join("git").join(&source.id),
                    pinned: if locked { pinned } else { None },
                })
            }
            SourceKind::Ods { project_dir, target_dir, command, infer } => Box::new(onto_ods::OdsSource {
                id: source.id.clone(),
                layer: source.layer,
                command: command.clone(),
                project_dir: self.root.join(project_dir),
                target_dir: target_dir.as_ref().map(|t| self.root.join(t)),
                infer: *infer,
            }),
            SourceKind::Command { command, authority } => {
                Box::new(CommandSource { id: source.id.clone(), layer: source.layer, command: command.clone(), cwd: self.root.clone(), authority: *authority })
            }
        })
    }

    /// Sync configured sources into `store`. A failing source keeps its previous
    /// generation and is reported; other sources continue.
    pub fn sync(&self, store: &mut Store, options: &SyncOptions) -> Result<Vec<SyncReport>> {
        let lock = Lock::read(&self.root)?;
        let mut new_lock = Lock { version: 1, sources: Vec::new() };
        let mut reports = Vec::new();

        for source in &self.config.sources {
            let selected = options.only.as_ref().is_none_or(|only| only.contains(&source.id));
            if !selected {
                if let Some(previous) = lock.as_ref().and_then(|l| l.get(&source.id, url_of(source))) {
                    new_lock.sources.push(previous.clone());
                }
                continue;
            }
            let outcome = match self.sync_one(store, source, lock.as_ref(), options.locked) {
                Ok((version, records)) => {
                    if let (SourceKind::Git { url, .. }, Some(commit)) = (&source.kind, &version) {
                        new_lock.sources.push(LockedSource { id: source.id.clone(), url: url.clone(), commit: commit.clone() });
                    }
                    SyncOutcome::Synced { version, records }
                }
                Err(error) => {
                    if let Some(previous) = lock.as_ref().and_then(|l| l.get(&source.id, url_of(source))) {
                        new_lock.sources.push(previous.clone());
                    }
                    SyncOutcome::Failed { error: error.to_string() }
                }
            };
            reports.push(SyncReport { source_id: source.id.clone(), outcome });
        }

        // A source disabled on this machine keeps its pin for everyone else.
        if let Some(lock) = &lock {
            for previous in lock.sources.iter().filter(|s| self.disabled.contains(&s.id)) {
                new_lock.sources.push(previous.clone());
            }
        }

        for stale in store.sources()? {
            if self.source_config(&stale.source_id).is_none() {
                store.remove_source(&stale.source_id)?;
            }
        }
        if !options.locked && lock.as_ref() != Some(&new_lock) && (!new_lock.sources.is_empty() || lock.is_some()) {
            new_lock.write(&self.root)?;
        }
        Ok(reports)
    }

    fn sync_one(&self, store: &mut Store, config: &SourceConfig, lock: Option<&Lock>, locked: bool) -> Result<(Option<String>, usize)> {
        self.check_trust(config)?;
        let source = self.build(config, lock, locked)?;
        let retrieved_at = now_rfc3339();
        let mut records = Vec::new();
        let mut cursor: Option<String> = None;
        let mut version = None;
        for _ in 0..MAX_PAGES {
            let page = source.fetch(cursor.as_deref()).map_err(|e| WorkspaceError::Config(e.to_string()))?;
            version = page.version.or(version);
            for mut record in page.records {
                if record.provenance.source_id != config.id || record.provenance.layer != config.layer {
                    return Err(WorkspaceError::Config(format!("source `{}` returned a record attributed to `{}`", config.id, record.provenance.source_id)));
                }
                record.provenance.retrieved_at.get_or_insert_with(|| retrieved_at.clone());
                if record.provenance.source_version.is_none() {
                    record.provenance.source_version = version.clone();
                }
                records.push(record);
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => {
                    let state = SourceState {
                        source_id: config.id.clone(),
                        kind: config.kind.name().into(),
                        layer: config.layer,
                        version: version.clone(),
                        synced_at: retrieved_at,
                        record_count: records.len(),
                    };
                    store.replace_source(&state, &records)?;
                    return Ok((version, records.len()));
                }
            }
        }
        Err(WorkspaceError::Config(format!("source `{}` exceeded {MAX_PAGES} pages", config.id)))
    }

    /// The pack that receives accepted proposals.
    pub fn proposal_target(&self) -> Result<(&SourceConfig, PathBuf)> {
        let source = match &self.config.proposals.target {
            Some(id) => self.source_config(id).ok_or_else(|| WorkspaceError::Config(format!("proposal target `{id}` is not a configured source")))?,
            None => self
                .config
                .sources
                .iter()
                .find(|s| s.layer == Layer::Repository && matches!(s.kind, SourceKind::Pack { .. }))
                .ok_or_else(|| WorkspaceError::Config("no repository-layer pack source to receive proposals".into()))?,
        };
        let dir = self.pack_dir(source).ok_or_else(|| WorkspaceError::Config(format!("proposal target `{}` must be a pack source", source.id)))?;
        Ok((source, dir))
    }
}

fn url_of(source: &SourceConfig) -> &str {
    match &source.kind {
        SourceKind::Git { url, .. } => url,
        _ => "",
    }
}

/// Convenience for tests and tools: does `root` contain an `onto.toml`?
pub fn is_workspace(root: &Path) -> bool {
    root.join(CONFIG_FILE).is_file()
}
