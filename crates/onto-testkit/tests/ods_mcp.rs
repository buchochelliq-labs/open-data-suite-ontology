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
