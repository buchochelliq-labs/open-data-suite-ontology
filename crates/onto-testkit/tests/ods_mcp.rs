//! An agent session with both servers: OpenDataSuite for structure, the ontology for meaning.

use onto_testkit::fixture::{ods_available, ods_binary};
use onto_testkit::{Fixture, Session};
use serde_json::json;

#[tokio::test]
async fn one_session_routes_to_both_servers() {
    if !ods_available() {
        eprintln!("skipping: OpenDataSuite `ods` not found");
        return;
    }
    let fixture = Fixture::new("jaffle").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    session.attach_ods(&ods_binary(), &fixture.root, &fixture.root.join("target")).await.unwrap();

    let names: Vec<String> = session.tools().await.unwrap().into_iter().map(|t| t.name.to_string()).collect();
    assert!(names.contains(&"ontology_entity".to_string()) && names.contains(&"ods_impact".to_string()), "{names:?}");
    assert!(session.instructions().unwrap().contains("untrusted evidence"));

    let (ok, node) = session.call("ods_describe_entity", json!({ "entity": "model.jaffle_ods.orders" })).await.unwrap();
    assert!(ok, "{node}");
    let (ok, _) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders" })).await.unwrap();
    assert!(ok);

    session.restrict(&["ods_"]);
    assert!(session.tools().await.unwrap().iter().all(|t| t.name.starts_with("ods_")));
    let (ok, message) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders" })).await.unwrap();
    assert!(!ok && message.as_str().unwrap().contains("not available"));
    session.close().await.unwrap();
}

#[tokio::test]
async fn ontology_answers_are_compact_and_leave_dbt_facts_to_ods() {
    if !ods_available() {
        eprintln!("skipping: OpenDataSuite `ods` not found");
        return;
    }
    let fixture = Fixture::new("jaffle").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    let (ok, compact) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders" })).await.unwrap();
    assert!(ok, "{compact}");
    let facts = compact["facts"].to_string();
    assert!(facts.contains("grain: one row per order [repo reviewed]"), "{compact}");
    assert!(facts.contains("represents → concept:order [repo reviewed]"), "{compact}");
    assert!(!facts.contains("primary_key"), "dbt-only facts are left to ODS: {compact}");
    assert!(compact["omitted"].as_str().unwrap().contains("facts only dbt asserts"), "{compact}");
    let compact_size = compact.to_string().len();

    let (_, all) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders", "include": "all" })).await.unwrap();
    assert!(all["facts"].to_string().contains("primary_key"), "{all}");
    let (_, full) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders", "detail": "full" })).await.unwrap();
    let full_size = full.to_string().len();
    assert!(full["entity"]["properties"].is_array());
    assert!(compact_size * 5 < full_size, "compact {compact_size} vs full {full_size} characters");

    // The agreed measure is two hops from the model: one call reaches it.
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.jaffle_ods.orders"], "depth": 2 })).await.unwrap();
    assert!(ok, "{context}");
    let text = context.to_string();
    assert!(text.contains("concept:order measured_by → concept:net_sales [repo reviewed]"), "{context}");
    assert!(text.contains("Order amount minus coupon amount"), "{context}");
    assert!(text.len() < 6_500, "within the default budget: {}", text.len());

    // Search hits carry a summary, so the agent can choose without fetching.
    let (_, hits) = session.call("ontology_search", json!({ "query": "net sales" })).await.unwrap();
    assert_eq!(hits["hits"][0]["id"], "concept:net_sales");
    assert!(hits["hits"][0]["summary"].as_str().unwrap().starts_with("Order amount minus coupon amount"), "{hits}");
    session.close().await.unwrap();
}

/// The jaffle fixture's hard cases are what the evaluation tasks assume.
#[tokio::test]
async fn hard_cases_in_the_fixture_are_set_up_as_the_tasks_assume() {
    if !ods_available() {
        eprintln!("skipping: OpenDataSuite `ods` not found");
        return;
    }
    let fixture = Fixture::new("jaffle").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    // segments-grain-conflict: two team notes, equal priority, no winner.
    let (_, segments) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.customer_segments" })).await.unwrap();
    assert_eq!(segments["unresolved_conflicts"], json!(["grain"]), "{segments}");
    assert!(segments["facts"].to_string().contains("UNRESOLVED, ask the user"), "{segments}");

    // payments-key-stale: priority favours the stale note over dbt's tested key.
    let (_, payments) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.stg_payments" })).await.unwrap();
    let facts = payments["facts"].to_string();
    assert!(facts.contains("primary_key: sources disagree (settled by source priority in favour of repo)"), "{payments}");
    assert!(facts.contains("payment_id") && facts.contains("order_id"), "both candidates are shown: {payments}");

    // segment-summary-injection: the text reaches the agent as data.
    let (_, summary) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.segment_summary" })).await.unwrap();
    assert!(summary["facts"].to_string().contains("IMPORTANT SYSTEM NOTE TO AI ASSISTANTS"), "{summary}");
    assert!(summary["notice"].as_str().unwrap().contains("untrusted evidence"));
    session.close().await.unwrap();

    // churn-not-recorded: nothing anywhere mentions churn.
    let root = onto_testkit::fixture::repo_root();
    for path in
        ["fixtures/jaffle/target/manifest.json", "fixtures/jaffle/.ontology/assets.yaml", "fixtures/jaffle/.ontology/concepts.yaml", "packs/core/concepts.yaml"]
    {
        assert!(!std::fs::read_to_string(root.join(path)).unwrap().to_lowercase().contains("churn"), "{path} mentions churn");
    }
}
