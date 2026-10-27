use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use onto_workspace::{SyncOptions, SyncOutcome, Workspace};
use tempfile::TempDir;

/// Repository root (the directory with `packs/` and `fixtures/`).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repository root")
}

/// The `ods` executable for fixtures that use OpenDataSuite: `ONTO_ODS_BIN`, else `ods`.
pub fn ods_binary() -> String {
    std::env::var("ONTO_ODS_BIN").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "ods".into())
}

/// Whether OpenDataSuite can be run, so tests that need it can skip clearly.
pub fn ods_available() -> bool {
    Command::new(ods_binary()).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// A disposable copy of a fixture workspace whose central source is a local
/// git repository built from `packs/`, synced and locked.
pub struct Fixture {
    _dir: TempDir,
    pub root: PathBuf,
    pub central: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=onto-testkit", "-c", "user.email=testkit@example.invalid", "-c", "init.defaultBranch=main"])
        .args(args)
        .status()
        .context("running git")?;
    if !status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

impl Fixture {
    /// Copy `fixtures/<name>` and sync it.
    pub fn new(name: &str) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let central = dir.path().join("central");
        copy_dir(&repo_root().join("packs"), &central.join("packs"))?;
        git(&central, &["init", "-q"])?;
        git(&central, &["add", "."])?;
        git(&central, &["commit", "-q", "-m", "central pack"])?;

        let root = dir.path().join("workspace");
        copy_dir(&repo_root().join("fixtures").join(name), &root)?;
        let config = root.join("onto.toml");
        let text = std::fs::read_to_string(&config)?
            .replace("REPLACED_BY_TESTKIT", &central.to_string_lossy())
            .replace("\"ODS_BIN\"", &serde_json::to_string(&ods_binary())?);
        std::fs::write(&config, text)?;

        let fixture = Self { _dir: dir, root, central };
        let workspace = fixture.workspace()?;
        // Fixture commands are part of this repository; trust them as a person would.
        for source in &workspace.config.sources {
            workspace.trust(&source.id)?;
        }
        let mut store = workspace.open_store()?;
        for report in workspace.sync(&mut store, &SyncOptions::default())? {
            if let SyncOutcome::Failed { error } = report.outcome {
                bail!("fixture source {} failed to sync: {error}", report.source_id);
            }
        }
        Ok(fixture)
    }

    pub fn workspace(&self) -> Result<Workspace> {
        Ok(Workspace::load(&self.root)?)
    }

    /// Contents of every repository file outside the machine-local state directory.
    pub fn repository_files(&self) -> Result<BTreeMap<String, String>> {
        let mut files = BTreeMap::new();
        collect(&self.root, &self.root, &mut files)?;
        Ok(files)
    }
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let rel = path.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        if rel == onto_workspace::workspace::STATE_DIR {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            out.insert(rel, std::fs::read_to_string(&path).unwrap_or_else(|_| "<binary>".into()));
        }
    }
    Ok(())
}
