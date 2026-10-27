//! ontology_context: one call for several entities and their relationships,
//! within a character budget that covers everything it returns.

use onto_testkit::{Fixture, Session};
use serde_json::json;

#[tokio::test]
async fn relationships_that_differ_only_in_qualifiers_are_both_kept() {
    let fixture = Fixture::new("wide").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.wide.payments"] })).await.unwrap();
    assert!(ok, "{context}");
    let relationships = context["relationships"].to_string();
    assert!(relationships.contains("references → model.wide.customers (column: payer_id)"), "{context}");
    assert!(relationships.contains("references → model.wide.customers (column: payee_id)"), "{context}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn the_budget_covers_relationships_and_keeps_the_starting_entity() {
    let fixture = Fixture::new("wide").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    let (_, small) = session.call("ontology_context", json!({ "ids": ["concept:hub"], "max_chars": 1000 })).await.unwrap();
    assert_eq!(small["entities"][0]["id"], "concept:hub", "the starting entity comes first: {small}");
    let listed: usize = small["entities"].to_string().len() + small["relationships"].to_string().len();
    assert!(listed <= 1_000 + 200, "entities and relationships stay near the budget: {listed}");
    assert!(small["omitted"].as_str().unwrap().contains("relationships did not fit the 1000-character budget"), "{small}");

    // Past the per-entity cap, the rest is counted and the agent pointed elsewhere.
    let (_, large) = session.call("ontology_context", json!({ "ids": ["concept:hub"], "max_chars": 20000 })).await.unwrap();
    assert!(large["omitted"].as_str().unwrap().contains("more than 200 relationships"), "{large}");
    assert!(large["relationships"].as_array().unwrap().len() <= 200);
    session.close().await.unwrap();
}
