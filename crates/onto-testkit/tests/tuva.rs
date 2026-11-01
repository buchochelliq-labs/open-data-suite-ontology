//! The Tuva fixture: a real dbt project (the Tuva Project) with a made-up
//! team layer. Its manifest is built by `fixtures/tuva/build.sh` (in CI);
//! without it these tests skip.

use onto_testkit::fixture::{fixture_not_built, ods_available};
use onto_testkit::{Fixture, Session};
use serde_json::json;

fn ready() -> bool {
    ods_available() && fixture_not_built("tuva").is_none()
}

#[test]
fn every_model_the_team_describes_exists_in_tuva() {
    if !ready() {
        return;
    }
    let fixture = Fixture::new("tuva").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let pack = std::fs::read_dir(fixture.root.join(".ontology")).unwrap();
    let mut ids = Vec::new();
    for file in pack {
        let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
        ids.extend(text.split(|c: char| c.is_whitespace() || c == ',' || c == '}').filter(|w| w.starts_with("model.")).map(str::to_string));
    }
    assert!(!ids.is_empty());
    for id in ids {
        // The team's pack names it; ODS must know it too, or the id is wrong.
        let records = store.records_about(&id).unwrap();
        assert!(records.iter().any(|r| r.provenance.source_id == "dbt"), "{id} is not in Tuva's manifest");
    }
}

#[tokio::test]
async fn pmpm_reaches_the_runout_rule_from_core_cost() {
    if !ready() {
        return;
    }
    let fixture = Fixture::new("tuva").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.the_tuva_project.core__cost"] })).await.unwrap();
    assert!(ok, "{context}");
    let text = context.to_string();
    assert!(text.contains("medical plus pharmacy"), "{text}");
    assert!(text.contains("latest three"), "{text}");
    session.close().await.unwrap();
}
