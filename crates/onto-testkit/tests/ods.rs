//! The ontology complementing OpenDataSuite: ODS supplies structure with evidence,
//! the repository adds meaning, and the agent sees both through one MCP server.
//!
//! Needs the `ods` CLI (`pip install opendatasuite==0.0.2`, or `ONTO_ODS_BIN`).
//! CI installs it; locally the test skips with a message when it is missing.

use onto_testkit::fixture::ods_available;
use onto_testkit::{Fixture, Session, invariants};
use serde_json::{Value, json};

fn property<'a>(entity: &'a Value, predicate: &str) -> &'a Value {
    entity["entity"]["properties"].as_array().unwrap().iter().find(|p| p["predicate"] == predicate).unwrap_or_else(|| panic!("no {predicate} in {entity}"))
}

#[tokio::test]
async fn ods_structure_and_repository_meaning_meet() {
    if !ods_available() {
        eprintln!("skipping: OpenDataSuite `ods` not found (pip install opendatasuite==0.0.2, or set ONTO_ODS_BIN)");
        return;
    }
    let fixture = Fixture::new("jaffle").unwrap();
    let before = fixture.repository_files().unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    let (_, status) = session.call("ontology_status", json!({})).await.unwrap();
    let dbt = status["sources"].as_array().unwrap().iter().find(|s| s["source_id"] == "dbt").unwrap();
    assert_eq!(dbt["kind"], "ods");
    assert!(dbt["record_count"].as_u64().unwrap() > 50);

    // Structure from ODS, with its evidence; meaning from the repository.
    let (_, orders) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders" })).await.unwrap();
    let pk = &property(&orders, "primary_key")["values"][0];
    assert_eq!(pk["object"], json!({ "literal": ["order_id"] }));
    assert_eq!(pk["qualifiers"]["basis"], "declared");
    assert_eq!(pk["sources"][0]["source_id"], "dbt");
    assert_eq!(pk["sources"][0]["authority"], "declared");
    assert_eq!(property(&orders, "grain")["values"][0]["sources"][0]["source_id"], "repo");
    assert_eq!(property(&orders, "represents")["values"][0]["object"]["entity"], "concept:order");

    // A tested relationship stays `observed`, with the test as evidence.
    let refs = property(&orders, "references")["values"].as_array().unwrap();
    let to_customers = refs.iter().find(|v| v["object"]["entity"] == "model.jaffle_ods.customers").unwrap();
    assert_eq!(to_customers["sources"][0]["authority"], "observed");
    assert!(to_customers["sources"][0]["rationale"].as_str().unwrap().contains("test.jaffle_ods.relationships"));

    // ODS's documented description and the team's definition disagree: priority
    // picks the team's, and ODS's stays visible as a candidate.
    let (_, customers) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.customers" })).await.unwrap();
    let definition = property(&customers, "onto:definition");
    assert_eq!(definition["conflict"]["status"], "resolved_by_priority");
    assert_eq!(definition["conflict"]["selected_source"], "repo");
    assert!(definition["conflict"]["candidates"].to_string().contains("One row per customer with order statistics."));

    // From a business concept to the physical structure ODS found.
    let (_, around) = session.call("ontology_neighbors", json!({ "id": "concept:order", "depth": 2 })).await.unwrap();
    let edges = around["edges"].to_string();
    assert!(edges.contains("model.jaffle_ods.order_events"), "both assets representing the concept");
    assert!(edges.contains("\"predicate\":\"references\""), "ODS key relationships one hop further");

    // Columns share ODS's ids.
    let (_, column) = session.call("ontology_entity", json!({ "id": "model.jaffle_ods.orders#customer_id" })).await.unwrap();
    assert_eq!(column["found"], true);
    assert!(property(&column, "data_type")["values"][0]["object"]["literal"].is_string());

    let (_, hits) = session.call("ontology_search", json!({ "query": "customer order rank" })).await.unwrap();
    assert_eq!(hits["hits"][0]["id"], "model.jaffle_ods.customer_order_rank");

    let transcript = session.close().await.unwrap();
    invariants::check(&transcript, &before, &fixture.repository_files().unwrap()).assert_clean();
}

#[test]
fn a_missing_ods_is_reported_and_other_sources_still_sync() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".ontology")).unwrap();
    std::fs::write(dir.path().join(".ontology/a.yaml"), "entities:\n  - { id: concept:x, label: X }\n").unwrap();
    std::fs::write(
        dir.path().join("onto.toml"),
        "[[source]]\nid = \"dbt\"\nlayer = \"repository\"\nkind = \"ods\"\ncommand = [\"definitely-not-ods\"]\n\n\
         [[source]]\nid = \"repo\"\nlayer = \"repository\"\nkind = \"pack\"\npath = \".ontology\"\n",
    )
    .unwrap();
    let workspace = onto_workspace::Workspace::load(dir.path()).unwrap();
    workspace.trust("dbt").unwrap();
    let mut store = workspace.open_store().unwrap();
    let reports = workspace.sync(&mut store, &Default::default()).unwrap();
    let failed = serde_json::to_string(&reports[0]).unwrap();
    assert!(failed.contains("failed") && failed.contains("pip install opendatasuite"), "{failed}");
    assert!(serde_json::to_string(&reports[1]).unwrap().contains("synced"));
}
