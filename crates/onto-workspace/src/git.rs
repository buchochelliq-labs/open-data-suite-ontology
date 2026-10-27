//! Central packs consumed from a git repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use onto_core::{Layer, Page, Source, SourceError, SourceInfo};

use crate::pack::PackSource;

pub struct GitSource {
    pub id: String,
    pub layer: Layer,
    pub url: String,
    pub rev: String,
    /// Pack directory inside the repository.
    pub path: String,
    /// Local clone, owned by this workspace's state directory.
    pub checkout: PathBuf,
    /// Commit from `onto.lock`; when set, `rev` is ignored.
    pub pinned: Option<String>,
}

impl GitSource {
    fn unavailable(&self, message: impl Into<String>) -> SourceError {
        SourceError::Unavailable { source_id: self.id.clone(), message: message.into() }
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Result<String, SourceError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            // Never run hooks or prompt for credentials while syncing.
            .args(["-c", "core.hooksPath=/dev/null", "-c", "advice.detachedHead=false"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| self.unavailable(format!("cannot run git: {e}")))?;
        if !output.status.success() {
            return Err(self.unavailable(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim())));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Fetch and check out the pinned commit (or `rev`), returning the commit.
    pub fn checkout(&self) -> Result<String, SourceError> {
        if !self.checkout.join(".git").is_dir() {
            std::fs::create_dir_all(&self.checkout).map_err(|e| self.unavailable(e.to_string()))?;
            self.git(&self.checkout, &["init", "-q"])?;
            self.git(&self.checkout, &["remote", "add", "origin", &self.url])?;
        } else {
            self.git(&self.checkout, &["remote", "set-url", "origin", &self.url])?;
        }
        self.git(&self.checkout, &["fetch", "-q", "--tags", "--force", "origin", "+refs/heads/*:refs/remotes/origin/*"])?;

        let wanted = match &self.pinned {
            Some(commit) => commit.clone(),
            None => {
                let remote_branch = format!("refs/remotes/origin/{}^{{commit}}", self.rev);
                match self.git(&self.checkout, &["rev-parse", "--verify", "-q", &remote_branch]) {
                    Ok(commit) => commit,
                    Err(_) => format!("{}^{{commit}}", self.rev),
                }
            }
        };
        let commit = self.git(&self.checkout, &["rev-parse", "--verify", &wanted])?;
        self.git(&self.checkout, &["checkout", "-q", "--force", "--detach", &commit])?;
        Ok(commit)
    }
}

impl Source for GitSource {
    fn info(&self) -> SourceInfo {
        SourceInfo { id: self.id.clone(), kind: "git".into(), layer: self.layer }
    }

    fn fetch(&self, cursor: Option<&str>) -> Result<Page, SourceError> {
        let commit = self.checkout()?;
        let pack =
            PackSource { id: self.id.clone(), layer: self.layer, kind: "git".into(), dir: self.checkout.join(&self.path), version: Some(commit.clone()) };
        let mut page = pack.fetch(cursor)?;
        for record in &mut page.records {
            record.provenance.source_version = Some(commit.clone());
        }
        Ok(page)
    }
}
