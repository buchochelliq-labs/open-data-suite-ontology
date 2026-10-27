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
