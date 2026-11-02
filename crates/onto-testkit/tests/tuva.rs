//! The Tuva fixture: a real dbt project (the Tuva Project) with a made-up
//! team layer. Its manifest is built by `fixtures/tuva/build.sh` (in CI);
//! without it these tests skip.

use onto_testkit::fixture::{fixture_not_built, ods_available, repo_root};
use onto_testkit::{Fixture, Session};
use serde_json::{Value, json};

fn ready() -> bool {
    ods_available() && fixture_not_built("tuva").is_none()
}

/// The dbt model ids the team's pack (`.ontology/`) names.
fn team_model_ids() -> Vec<String> {
    let pack = std::fs::read_dir(repo_root().join("fixtures/tuva/.ontology")).unwrap();
    let mut ids = Vec::new();
    for file in pack {
        let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
        ids.extend(text.split(|c: char| c.is_whitespace() || c == ',' || c == '}').filter(|w| w.starts_with("model.")).map(str::to_string));
    }
    ids.sort();
    ids.dedup();
    assert!(!ids.is_empty());
    ids
}

#[test]
fn the_models_the_team_describes_are_compiled() {
    // The manifest comes from `dbt compile`, so ODS can read each model's SQL
    // (its joins) rather than treating it as opaque. Needs no ODS.
    if fixture_not_built("tuva").is_some() {
        return;
    }
    let path = repo_root().join("fixtures/tuva/target/manifest.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for id in team_model_ids() {
        let node = &manifest["nodes"][&id];
        assert!(node.is_object(), "{id} is not in Tuva's manifest");
        let sql = node["compiled_code"].as_str().unwrap_or_default();
        assert!(!sql.trim().is_empty(), "{id} has no compiled SQL: was the manifest built with `dbt parse`?");
    }
}

#[test]
fn every_model_the_team_describes_exists_in_tuva() {
    if !ready() {
        return;
    }
    let fixture = Fixture::new("tuva").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    for id in team_model_ids() {
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

#[test]
fn compiled_sql_gives_ods_the_joins_between_tuva_models() {
    // With only `dbt parse`, every model was opaque to ODS: no relationships.
    // Compiled SQL lets ODS read the joins, recorded as `references`.
    if !ready() {
        return;
    }
    let fixture = Fixture::new("tuva").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let references = store.records_with_predicate("references").unwrap();
    let tuva: Vec<_> = references.iter().filter(|r| r.claim.subject.starts_with("model.the_tuva_project.")).collect();
    for r in tuva.iter().filter(|r| r.claim.subject.ends_with("core__cost")) {
        println!("{} -> {:?} {:?}", r.claim.subject, r.claim.object, r.claim.qualifiers);
    }
    println!("{} relationships between Tuva models", tuva.len());
    assert!(!tuva.is_empty(), "ODS found no joins in Tuva's compiled SQL");
}

#[tokio::test]
async fn the_open_member_month_issue_is_listed_on_its_model_and_in_the_digest() {
    if !ready() {
        return;
    }
    let fixture = Fixture::new("tuva").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, entity) = session.call("ontology_entity", json!({ "id": "model.the_tuva_project.core__member_month" })).await.unwrap();
    assert!(ok, "{entity}");
    assert!(entity["known_issues"].to_string().contains("issue:RB-204"), "{entity}");
    session.close().await.unwrap();

    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let digest = onto_workspace::digest::render(&workspace, &store, &onto_workspace::digest::DigestOptions::default()).unwrap();
    assert!(digest.contains("Known issues") && digest.contains("RB-204"), "{digest}");
}
