//! The pilot fixture: a made-up B2B SaaS dbt project (billing, product usage,
//! CRM) whose team facts span finance, product and sales.

use onto_testkit::fixture::ods_available;
use onto_testkit::{Fixture, Session};
use onto_workspace::digest::{DigestOptions, render};
use serde_json::json;

#[test]
fn the_saas_digest_carries_the_teams_definitions_and_disagreements() {
    if !ods_available() {
        return;
    }
    let fixture = Fixture::new("saas").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let text = render(&workspace, &store, &DigestOptions::default()).unwrap();
    assert!(text.contains("Annual plans count at one twelfth"), "{text}");
    assert!(text.contains("sources disagree on definition"), "the ARR definitions conflict: {text}");
    assert!(text.contains("DATA-412"), "{text}");
    assert!(text.contains("`model.acme.mrr_legacy`"), "{text}");
    println!("{text}");
}

#[tokio::test]
async fn arr_is_unresolved_and_reachable_from_the_board_model() {
    if !ods_available() {
        return;
    }
    let fixture = Fixture::new("saas").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.acme.rpt_board_metrics"], "depth": 2 })).await.unwrap();
    assert!(ok, "{context}");
    let text = context.to_string();
    assert!(text.contains("one row per calendar month"), "{text}");
    assert!(text.contains("UNRESOLVED"), "ARR's two definitions are shown, not settled: {text}");
    assert!(text.contains("one twelfth"), "MRR is two hops away: {text}");
    println!("{}", serde_json::to_string_pretty(&context).unwrap());
    session.close().await.unwrap();
}
