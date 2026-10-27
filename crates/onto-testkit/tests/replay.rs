//! Tier 3 feeds tier 1: a recorded transcript replays deterministically.

use onto_testkit::fixture::repo_root;
use onto_testkit::scenario::Scenario;
use onto_testkit::{Fixture, Session, Transcript, replay};

#[tokio::test]
async fn recorded_transcripts_replay_on_a_fresh_fixture() {
    let scenario = Scenario::load(&repo_root().join("scenarios/propose-and-accept.yaml")).unwrap();
    let recorded = scenario.run().await.unwrap();
    let recorded = Transcript::from_jsonl(&recorded.to_jsonl()).unwrap();

    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let mismatches = replay(&mut session, &recorded).await.unwrap();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    assert_eq!(session.close().await.unwrap().events.len(), recorded.events.len());
}

#[tokio::test]
async fn replay_detects_behaviour_changes() {
    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    session.call("ontology_search", serde_json::json!({"query": "client"})).await.unwrap();
    let mut recorded = session.close().await.unwrap();
    if let onto_testkit::Event::ToolCall { result, .. } = &mut recorded.events[0] {
        result["hits"][0]["id"] = "concept:somebody-else".into();
    }

    let mut session = Session::start(&fixture).await.unwrap();
    assert_eq!(replay(&mut session, &recorded).await.unwrap().len(), 1);
}

/// Conversations recorded from a live evaluation (`evals/recorded/`) replay
/// their ontology calls on every change, at no token cost. A mismatch means
/// the ontology now answers differently: check it, then re-record live.
#[tokio::test]
async fn recorded_evaluation_conversations_still_replay() {
    let dir = repo_root().join("evals/recorded");
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")).collect();
    paths.sort();
    assert!(!paths.is_empty(), "no recorded conversations in {}", dir.display());
    for path in paths {
        let recorded = Transcript::from_jsonl(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // ODS calls belong to OpenDataSuite; status carries sync times and versions.
        let ontology = Transcript {
            events: recorded
                .events
                .into_iter()
                .filter(|e| match e {
                    onto_testkit::Event::ToolCall { tool, .. } => tool.starts_with("ontology_") && tool != "ontology_status",
                    _ => true,
                })
                .collect(),
        };
        let fixture = Fixture::new("jaffle").unwrap();
        let mut session = Session::start(&fixture).await.unwrap();
        let mismatches = replay(&mut session, &ontology).await.unwrap();
        assert!(mismatches.is_empty(), "{}:\n{}", path.display(), mismatches.join("\n"));
        session.close().await.unwrap();
    }
}
