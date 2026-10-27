//! Per-person settings and the line between shared and machine-local state.
//!
//! `onto.toml`, `onto.lock` and authored packs are shared through git. Facts
//! from external sources are not: every person syncs them into their own
//! `.onto/` with their own access, so nobody sees facts they could not read
//! themselves and nothing synced is ever committed. `.onto/local.toml` adjusts
//! sources for one machine without changing the shared configuration or lock.
//!
//! ```toml
//! [[source]]
//! id = "catalog"
//! enabled = false            # no access here; skip it
//!
//! [[source]]
//! id = "dbt"
//! command = ["/opt/ods/bin/ods"]
//! target_dir = "../shop/target"
//! ```

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::config::{Config, SourceKind};
use crate::workspace::{STATE_DIR, WorkspaceError};

pub const LOCAL_CONFIG: &str = "local.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    #[serde(default, rename = "source")]
    pub sources: Vec<LocalSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSource {
    pub id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Executable for `ods` and `command` sources.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// For `ods` sources.
    #[serde(default)]
    pub project_dir: Option<String>,
    /// For `ods` sources.
    #[serde(default)]
    pub target_dir: Option<String>,
}

impl LocalConfig {
    pub fn read(root: &Path) -> Result<Self, WorkspaceError> {
        let path = root.join(STATE_DIR).join(LOCAL_CONFIG);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).map_err(|e| WorkspaceError::Config(format!("{}: {e}", path.display())))
    }

    /// Apply to the shared config. Returns the ids of sources disabled here.
    pub fn apply(self, config: &mut Config) -> Result<Vec<String>, WorkspaceError> {
        let mut disabled = Vec::new();
        for local in self.sources {
            let source = config
                .sources
                .iter_mut()
                .find(|s| s.id == local.id)
                .ok_or_else(|| WorkspaceError::Config(format!("{LOCAL_CONFIG} names unknown source `{}`", local.id)))?;
            if local.enabled == Some(false) {
                disabled.push(local.id.clone());
            }
            let kind = source.kind.name();
            let not_applicable = |field: &str| WorkspaceError::Config(format!("{LOCAL_CONFIG}: `{field}` does not apply to {kind} source `{}`", local.id));
            match &mut source.kind {
                SourceKind::Ods { project_dir, target_dir, command, .. } => {
                    if let Some(c) = local.command {
                        *command = c;
                    }
                    if let Some(p) = local.project_dir {
                        *project_dir = p;
                    }
                    if local.target_dir.is_some() {
                        *target_dir = local.target_dir;
                    }
                }
                SourceKind::Command { command, .. } => {
                    if local.project_dir.is_some() || local.target_dir.is_some() {
                        return Err(not_applicable("project_dir/target_dir"));
                    }
                    if let Some(c) = local.command {
                        *command = c;
                    }
                }
                _ if local.command.is_some() || local.project_dir.is_some() || local.target_dir.is_some() => {
                    return Err(not_applicable("command/project_dir/target_dir"));
                }
                _ => {}
            }
        }
        config.sources.retain(|s| !disabled.contains(&s.id));
        config.resolution.priority.retain(|id| !disabled.contains(id));
        Ok(disabled)
    }
}

/// Whether machine-local state is git-ignored: `None` outside a git repository.
pub fn state_dir_ignored(root: &Path) -> Option<bool> {
    // The trailing slash lets `.onto/` patterns match before the directory exists.
    let output = Command::new("git").arg("-C").arg(root).args(["check-ignore", "-q", &format!("{STATE_DIR}/")]).output().ok()?;
    match output.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Add `.onto/` to the repository's `.gitignore` if it is not ignored yet.
pub fn ignore_state_dir(root: &Path) -> std::io::Result<bool> {
    if state_dir_ignored(root) == Some(true) {
        return Ok(false);
    }
    let path = root.join(".gitignore");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.lines().any(|l| l.trim() == ".onto/" || l.trim() == "/.onto/") {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# Machine-local ontology state: synced external facts, pending proposals.\n.onto/\n");
    std::fs::write(path, text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_is_added_once_and_detected_by_git() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Command::new("git").arg("-C").arg(dir.path()).args(["init", "-q"]).status().unwrap().success());
        std::fs::write(dir.path().join(".gitignore"), "target").unwrap();
        assert_eq!(state_dir_ignored(dir.path()), Some(false));
        assert!(ignore_state_dir(dir.path()).unwrap());
        assert!(!ignore_state_dir(dir.path()).unwrap());
        assert_eq!(state_dir_ignored(dir.path()), Some(true));
        assert!(std::fs::read_to_string(dir.path().join(".gitignore")).unwrap().starts_with("target\n"));
    }

    #[test]
    fn outside_git_the_answer_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(state_dir_ignored(dir.path()), None);
    }
}
