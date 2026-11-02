//! `onto init --agents`: one command connects coding agents, and `onto sync`
//! keeps each person's digest fresh.

use std::path::Path;
use std::process::Command;

fn onto(root: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_onto")).arg("--root").arg(root).args(args).output().unwrap();
    assert!(out.status.success(), "onto {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn init_connects_every_agent_and_sync_writes_the_digest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("dbt_project.yml"), "name: shop\n").unwrap();
    // An existing MCP config and agent notes are kept.
    std::fs::write(root.join(".mcp.json"), r#"{"mcpServers": {"github": {"command": "gh-mcp"}}}"#).unwrap();
    std::fs::write(root.join("AGENTS.md"), "# Agents\n\nRun the tests before pushing.\n").unwrap();

    onto(root, &["init", "--agents", "claude-code,copilot,generic"]);

    let mcp: serde_json::Value = serde_json::from_str(&read(root, ".mcp.json")).unwrap();
    assert_eq!(mcp["mcpServers"]["github"]["command"], "gh-mcp", "existing servers are kept");
    assert_eq!(mcp["mcpServers"]["onto"]["args"], serde_json::json!(["mcp"]));
    assert_eq!(mcp["mcpServers"]["ods"]["args"], serde_json::json!(["mcp"]), "a dbt project gets ods too");
    assert!(read(root, ".claude/skills/ontology-grounding/SKILL.md").starts_with("---\nname: ontology-grounding"));
    assert!(read(root, "CLAUDE.md").contains("@.onto/context.md"));
    let vscode: serde_json::Value = serde_json::from_str(&read(root, ".vscode/mcp.json")).unwrap();
    assert_eq!(vscode["servers"]["onto"]["type"], "stdio");
    assert!(read(root, ".github/agents/ontology-modelling.agent.md").contains("ontology-grounding"));
    assert!(read(root, ".github/copilot-instructions.md").contains(".onto/context.md"));
    let agents = read(root, "AGENTS.md");
    assert!(agents.starts_with("# Agents\n\nRun the tests before pushing.") && agents.contains("`onto mcp` and `ods mcp`"), "{agents}");
    assert!(read(root, "onto.toml").contains("[context]\npath = \".onto/context.md\""));

    // Re-running replaces onto's sections instead of adding more.
    onto(root, &["init", "--agents", "generic,claude-code"]);
    assert_eq!(read(root, "AGENTS.md").matches("onto:begin").count(), 1);
    assert_eq!(read(root, "CLAUDE.md").matches("onto:begin").count(), 1);
    assert_eq!(read(root, "onto.toml").matches("[context]").count(), 1);

    // A team fact, then sync: the digest appears where the agents look.
    std::fs::write(
        root.join(".ontology/concepts.yaml"),
        "entities:\n  - id: concept:customer\n    type: BusinessConcept\n    label: Customer\n    definition: Someone with a paid order.\n",
    )
    .unwrap();
    let synced = onto(root, &["sync"]);
    assert!(synced.contains("wrote   digest"), "{synced}");
    assert!(read(root, ".onto/context.md").contains("Someone with a paid order."));
    assert!(read(root, ".gitignore").contains(".onto/"), "the digest is per person and never committed");
}

/// Codex review: a `[context]` table spelled differently is still the table.
#[test]
fn an_existing_context_table_is_recognised_however_it_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    onto(root, &["init"]);
    let text = read(root, "onto.toml").replace("[context]", "[ context ] # agent digest");
    std::fs::write(root.join("onto.toml"), text).unwrap();
    onto(root, &["init", "--agents", "generic"]);
    let text = read(root, "onto.toml");
    assert_eq!(text.matches("context ]").count() + text.matches("[context]").count(), 1, "{text}");
    onto(root, &["status"]);
}

#[test]
fn init_on_an_existing_workspace_needs_agents() {
    let dir = tempfile::tempdir().unwrap();
    onto(dir.path(), &["init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_onto")).arg("--root").arg(dir.path()).arg("init").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("pass --agents"));
}
