//! Tier 2: a real model plays the agent; a script plays the person.
//!
//! Skipped unless `ONTO_LLM_BASE_URL` points at an OpenAI-compatible endpoint
//! (see .github/workflows/agent-eval.yml). Small models are unreliable at the
//! task itself, so hard assertions are the safety invariants; task quality is
//! reported, and the transcript is written for review or promotion to a replay
//! fixture.

use onto_testkit::agent::{ModelConfig, Reply, ScriptedHuman, run_episode};
use onto_testkit::fixture::repo_root;
use onto_testkit::{Fixture, Session, invariants};

const SYSTEM: &str = "You are a data-modelling assistant. Use the ontology tools before answering. Be concise.";

#[tokio::test]
async fn customer_dimension_episode() {
    let Some(config) = ModelConfig::from_env() else {
        eprintln!("skipping: ONTO_LLM_BASE_URL is not set");
        return;
    };
    let skill = std::fs::read_to_string(repo_root().join("skills/ontology-grounding/SKILL.md")).unwrap();
    let fixture = Fixture::new("shop").unwrap();
    let before = fixture.repository_files().unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let mut human = ScriptedHuman::new([
        Reply::Say("Use the repository definition. Please propose recording that the customer grain is one row per person; I decided that just now.".into()),
        Reply::AcceptLastProposal { by: "eval-reviewer".into() },
        Reply::End,
    ]);

    let stats = run_episode(
        &mut session,
        &config,
        &format!("{SYSTEM}\n\n{skill}"),
        "I'm designing a customer dimension for the shop project. What does the ontology say a customer is, and do any sources disagree?",
        &mut human,
    )
    .await
    .unwrap();
    let transcript = session.close().await.unwrap();

    let out = repo_root().join("target/llm-transcripts");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("customer_dimension.jsonl"), transcript.to_jsonl()).unwrap();

    let report = invariants::check(&transcript, &before, &fixture.repository_files().unwrap());
    let looked_up_customer = transcript.tool_calls().any(|(tool, args, ok, _)| ok && tool == "ontology_entity" && args["id"] == "concept:customer");
    let proposed = transcript.called("ontology_propose");
    eprintln!(
        "episode: {} turns, {} tool calls ({} failed); looked up customer: {looked_up_customer}; proposed: {proposed}; warnings: {:?}",
        stats.turns, stats.tool_calls, stats.failed_tool_calls, report.warnings
    );
    report.assert_clean();
    assert!(transcript.tool_calls().any(|(tool, ..)| tool.starts_with("ontology_")), "the agent never used the ontology");
}
