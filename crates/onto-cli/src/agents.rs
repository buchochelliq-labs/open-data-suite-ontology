//! `onto init --agents`: connect coding agents to the ontology.
//!
//! Each agent gets the `onto` MCP server (and OpenDataSuite's `ods` when the
//! repository is a dbt project), the skills, and a pointer to the context
//! digest. The digest is per person: `onto sync` writes it to
//! `.onto/context.md`, which is git-ignored, so the shared files only point to
//! it. Setup is idempotent: JSON configs are merged and Markdown sections are
//! replaced between markers, so it can be re-run after an upgrade.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Where `onto sync` writes the digest, relative to the workspace root.
pub const DIGEST_PATH: &str = ".onto/context.md";

// Copies of the repository's skills/ and .github/agents/, inside the crate so the
// published package carries them (crates.io packages only the crate's directory).
// tests/assets.rs fails when a copy drifts from its source.
const GROUNDING_SKILL: &str = include_str!("../assets/skills/ontology-grounding/SKILL.md");
const STENO_SKILL: &str = include_str!("../assets/skills/steno/SKILL.md");
const MODELLING_AGENT: &str = include_str!("../assets/agents/ontology-modelling.agent.md");

const BEGIN: &str = "<!-- onto:begin (written by `onto init --agents`; edits inside are replaced) -->";
const END: &str = "<!-- onto:end -->";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Copilot,
    Generic,
}

impl std::str::FromStr for Agent {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "claude-code" => Ok(Agent::ClaudeCode),
            "copilot" => Ok(Agent::Copilot),
            "generic" => Ok(Agent::Generic),
            other => bail!("unknown agent `{other}`: use claude-code, copilot or generic"),
        }
    }
}

/// Add `onto` (and `ods` for a dbt project) to an MCP config's `key` object,
/// keeping every server already there.
fn merge_servers(path: &Path, key: &str, server: impl Fn(&str, &str) -> Value, with_ods: bool) -> Result<()> {
    let mut config: Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.into()),
    };
    let servers = config.as_object_mut().context("MCP config is not a JSON object")?.entry(key).or_insert_with(|| json!({}));
    let servers = servers.as_object_mut().with_context(|| format!("`{key}` in {} is not an object", path.display()))?;
    servers.entry("onto").or_insert_with(|| server("onto", "mcp"));
    if with_ods {
        servers.entry("ods").or_insert_with(|| server("ods", "mcp"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&config)? + "\n")?;
    Ok(())
}

/// Replace the marked section of a Markdown file, or append it.
fn upsert_section(path: &Path, body: &str) -> Result<()> {
    let section = format!("{BEGIN}\n{}\n{END}\n", body.trim_end());
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let updated = match (text.find(BEGIN), text.find(END)) {
        (Some(start), Some(end)) if end > start => format!("{}{section}{}", &text[..start], text[end + END.len()..].trim_start_matches('\n')),
        _ if text.is_empty() => section,
        _ => format!("{}\n\n{section}", text.trim_end()),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, updated)?;
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// What every agent is told about the ontology and where `onto sync` writes
/// its digest (the configured `[context] path`).
fn pointer(digest: &str) -> String {
    format!(
        "## Team ontology\n\n\
         This repository has a team ontology (`onto.toml`): agreed definitions, grains, \
         standards and decisions, each with its source. Ground data-modelling answers in \
         it through the `onto` MCP server (and OpenDataSuite's `ods` for the dbt \
         project's structure). Your own digest of the team's facts is in \
         `{digest}` after `onto sync`; it is per person and never committed."
    )
}

/// Set up `agent` in the workspace at `root`, pointing it at the digest
/// `onto sync` writes to `digest`; returns the files written.
pub fn setup(root: &Path, agent: Agent, digest: &str) -> Result<Vec<String>> {
    let pointer = pointer(digest);
    let with_ods = root.join("dbt_project.yml").exists();
    let mut written = Vec::new();
    match agent {
        Agent::ClaudeCode => {
            merge_servers(&root.join(".mcp.json"), "mcpServers", |cmd, arg| json!({ "command": cmd, "args": [arg] }), with_ods)?;
            write(&root.join(".claude/skills/ontology-grounding/SKILL.md"), GROUNDING_SKILL)?;
            write(&root.join(".claude/skills/steno/SKILL.md"), STENO_SKILL)?;
            // CLAUDE.md imports the digest; a teammate who has not synced yet
            // simply has no file to import.
            upsert_section(&root.join("CLAUDE.md"), &format!("{pointer}\n\n@{digest}"))?;
            written.extend([".mcp.json", ".claude/skills/ontology-grounding/SKILL.md", ".claude/skills/steno/SKILL.md", "CLAUDE.md"].map(String::from));
        }
        Agent::Copilot => {
            merge_servers(&root.join(".vscode/mcp.json"), "servers", |cmd, arg| json!({ "type": "stdio", "command": cmd, "args": [arg] }), with_ods)?;
            write(&root.join(".github/agents/ontology-modelling.agent.md"), MODELLING_AGENT)?;
            write(&root.join(".github/skills/ontology-grounding/SKILL.md"), GROUNDING_SKILL)?;
            write(&root.join(".github/skills/steno/SKILL.md"), STENO_SKILL)?;
            upsert_section(&root.join(".github/copilot-instructions.md"), &format!("{pointer} Read it before answering."))?;
            written.extend(
                [
                    ".vscode/mcp.json",
                    ".github/agents/ontology-modelling.agent.md",
                    ".github/skills/ontology-grounding/SKILL.md",
                    ".github/skills/steno/SKILL.md",
                    ".github/copilot-instructions.md",
                ]
                .map(String::from),
            );
        }
        Agent::Generic => {
            let servers = if with_ods { "`onto mcp` and `ods mcp`" } else { "`onto mcp`" };
            upsert_section(
                &root.join("AGENTS.md"),
                &format!(
                    "{pointer} Read it before answering.\n\nMCP servers (stdio): {servers}. Without MCP, `onto search`, \
                     `onto show <id>` and `onto neighbors <id>` read the same facts."
                ),
            )?;
            written.push("AGENTS.md".into());
        }
    }
    Ok(written)
}
