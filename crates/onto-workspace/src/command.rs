//! The universal adapter: any provider that can print JSON lines.
//!
//! Each stdout line is a [`CommandRecord`]. The command never chooses its own
//! `source_id` or layer; those come from `onto.toml`, so an adapter cannot
//! impersonate another source. Commands only run on an explicit `onto sync`,
//! never from an agent-facing tool.

use std::path::PathBuf;
use std::process::Command;

use onto_core::digest::hex_sha256;
use onto_core::{Authority, Claim, Layer, Page, Provenance, Record, Source, SourceError, SourceInfo};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRecord {
    pub claim: Claim,
    #[serde(default)]
    pub authority: Option<Authority>,
    #[serde(default)]
    pub locator: Option<String>,
    #[serde(default)]
    pub source_version: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
}

pub struct CommandSource {
    pub id: String,
    pub layer: Layer,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub authority: Authority,
}

impl Source for CommandSource {
    fn info(&self) -> SourceInfo {
        SourceInfo { id: self.id.clone(), kind: "command".into(), layer: self.layer }
    }

    fn fetch(&self, _cursor: Option<&str>) -> Result<Page, SourceError> {
        let unavailable = |message: String| SourceError::Unavailable { source_id: self.id.clone(), message };
        let (program, args) = self.command.split_first().ok_or_else(|| unavailable("empty command".into()))?;
        let output = Command::new(program).args(args).current_dir(&self.cwd).output().map_err(|e| unavailable(format!("cannot run {program}: {e}")))?;
        if !output.status.success() {
            return Err(unavailable(format!("{program} exited with {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim())));
        }
        let stdout = String::from_utf8(output.stdout).map_err(|e| SourceError::Invalid { source_id: self.id.clone(), message: e.to_string() })?;
        let mut records = Vec::new();
        for (index, line) in stdout.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let parsed: CommandRecord =
                serde_json::from_str(line).map_err(|e| SourceError::Invalid { source_id: self.id.clone(), message: format!("line {}: {e}", index + 1) })?;
            let mut provenance = Provenance::new(&self.id, self.layer, parsed.authority.unwrap_or(self.authority));
            provenance.locator = parsed.locator;
            provenance.source_version = parsed.source_version;
            provenance.rationale = parsed.rationale;
            records.push(Record { claim: parsed.claim, provenance });
        }
        Ok(Page { records, next_cursor: None, version: Some(format!("sha256:{}", hex_sha256(stdout.as_bytes()))) })
    }
}
