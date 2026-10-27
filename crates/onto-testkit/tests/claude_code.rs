//! The Claude Code agent's process plumbing, with a fake `claude` that
//! records its arguments and prints stream-json. No subscription is used.

#![cfg(feature = "llm")]

use std::os::unix::fs::PermissionsExt;

use onto_testkit::agent::{Reply, ScriptedHuman};
use onto_testkit::claude_code::{ClaudeCodeConfig, Server, run_episode};
use onto_testkit::{Fixture, Transcript};

const FAKE: &str = r#"#!/bin/sh
# Record the arguments, one per line, then answer like `claude -p --output-format stream-json`.
for a in "$@"; do printf '%s\n' "$a"; done >> "$FAKE_LOG"
echo "---" >> "$FAKE_LOG"
case " $* " in
  *" --resume "*)
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Noted; I proposed it for review."}]}}'
    echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1}'
    ;;
  *)
    echo '{"type":"system","subtype":"init","session_id":"sess-1","model":"fake","mcp_servers":[{"name":"onto","status":"connected"}]}'
    echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__onto__ontology_entity","input":{"id":"concept:customer"}}]}}'
    echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"{\"found\":true}","is_error":false}]}}'
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The repository says an account holder."}]}}'
    echo '{"type":"result","subtype":"success","is_error":false,"num_turns":2}'
    ;;
esac
"#;

#[tokio::test]
async fn claude_code_is_driven_through_its_cli() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("claude");
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = dir.path().join("args.log");
    // SAFETY: this test binary runs this test alone in its process; nothing else reads the variable concurrently.
    unsafe { std::env::set_var("FAKE_LOG", &log) };

    let fixture = Fixture::new("shop").unwrap();
    let config = ClaudeCodeConfig {
        bin: bin.to_string_lossy().into_owned(),
        model: Some("sonnet".into()),
        max_turns: 7,
        onto_bin: "/path/to/onto".into(),
        timeout: std::time::Duration::from_secs(20),
    };
    let servers = [Server::onto(&config, &fixture)];
    let mut transcript = Transcript::default();
    let mut human = ScriptedHuman::new([Reply::Say("Record grain as one row per person.".into()), Reply::End]);
    let stats = run_episode(&fixture, &servers, &config, "system prompt", "What is a customer?", &mut human, &mut transcript).await.unwrap();

    assert_eq!((stats.turns, stats.tool_calls), (3, 1));
    let (tool, ..) = transcript.tool_calls().next().unwrap();
    assert_eq!(tool, "ontology_entity");
    assert_eq!(transcript.agent_texts().collect::<Vec<_>>(), vec!["The repository says an account holder.", "Noted; I proposed it for review."]);

    let calls = std::fs::read_to_string(&log).unwrap();
    let calls: Vec<Vec<&str>> = calls.split("---\n").filter(|c| !c.is_empty()).map(|c| c.lines().collect()).collect();
    assert_eq!(calls.len(), 2, "one invocation, then one resume");
    let first = &calls[0];
    let value_after = |args: &[&str], flag: &str| args.iter().position(|a| *a == flag).map(|i| args[i + 1].to_string());
    assert_eq!(first[0], "-p");
    assert_eq!(first[1], "What is a customer?");
    for flag in ["--strict-mcp-config", "--verbose"] {
        assert!(first.contains(&flag), "missing {flag}");
    }
    assert_eq!(value_after(first, "--tools").as_deref(), Some(""), "built-in tools are disabled");
    assert_eq!(value_after(first, "--allowedTools").as_deref(), Some("mcp__onto"));
    assert_eq!(value_after(first, "--max-turns").as_deref(), Some("7"));
    assert_eq!(value_after(first, "--model").as_deref(), Some("sonnet"));
    let mcp = std::fs::read_to_string(value_after(first, "--mcp-config").unwrap());
    assert!(mcp.is_err(), "the MCP config lives in a scratch dir removed after the episode");
    assert_eq!(value_after(&calls[1], "--resume").as_deref(), Some("sess-1"));
    assert_eq!(calls[1][1], "Record grain as one row per person.");
}
