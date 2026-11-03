//! Team-wide standards (#46): a standard or decision with no `applies_to`
//! binds every model, so `ontology_context` and `ontology_entity` list it with
//! every answer, as the digest does. Without this, an agent that looks up the
//! models it is asked about never meets it.

use onto_testkit::{Fixture, Session};
use onto_workspace::{SyncOptions, SyncOutcome};
use serde_json::{Value, json};

fn add_pack(fixture: &Fixture, name: &str, yaml: &str) {
    std::fs::write(fixture.root.join(".ontology").join(name), yaml).unwrap();
    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    for report in workspace.sync(&mut store, &SyncOptions::default()).unwrap() {
        assert!(!matches!(report.outcome, SyncOutcome::Failed { .. }), "{}", report.source_id);
    }
}

fn lines(value: &Value) -> Vec<String> {
    value.as_array().map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect()).unwrap_or_default()
}

const STANDARDS: &str = r#"entities:
  - id: standard:count-people
    type: ModellingStandard
    label: Count people, not members
    definition: Count people with person_id; a person can have several member ids.
  - id: standard:order-grain
    type: ModellingStandard
    label: Order grain
    definition: Order models are one row per order.
    relations:
      - { predicate: applies_to, target: model.shop.fct_orders }
  - id: standard:naming-ours
    type: ModellingStandard
    label: Our naming
    definition: Staging models are named src_<source>__<entity>.
    relations:
      - { predicate: refines, target: standard:model-naming }
"#;

#[tokio::test]
async fn team_wide_standards_come_with_every_context_and_entity() {
    let fixture = Fixture::new("shop").unwrap();
    add_pack(&fixture, "standards.yaml", STANDARDS);
    let mut session = Session::start(&fixture).await.unwrap();

    // Asked about a concept no standard is linked to, the agent still gets them.
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["concept:return"] })).await.unwrap();
    assert!(ok, "{context}");
    let standards = lines(&context["team_standards"]);
    assert!(standards.iter().any(|l| l.starts_with("standard:count-people \"Count people, not members\": Count people with person_id")), "{standards:?}");
    assert!(standards.iter().any(|l| l.contains("[repo")), "each cites its source: {standards:?}");
    // Scoped to one model: reached through that model's relationships instead.
    assert!(!standards.iter().any(|l| l.starts_with("standard:order-grain")), "{standards:?}");
    // The core pack's model naming applies to any model, but the team refines it:
    // only the team's rule is listed.
    assert!(standards.iter().any(|l| l.starts_with("standard:naming-ours")), "{standards:?}");
    assert!(!standards.iter().any(|l| l.starts_with("standard:model-naming")), "{standards:?}");

    let (ok, entity) = session.call("ontology_entity", json!({ "id": "concept:return" })).await.unwrap();
    assert!(ok, "{entity}");
    assert!(lines(&entity["team_standards"]).iter().any(|l| l.starts_with("standard:count-people")), "{entity}");

    // Not repeated when the standard is itself what was asked for.
    let (_, context) = session.call("ontology_context", json!({ "ids": ["standard:count-people"] })).await.unwrap();
    assert!(!lines(&context["team_standards"]).iter().any(|l| l.starts_with("standard:count-people")), "{context}");
    let (_, entity) = session.call("ontology_entity", json!({ "id": "standard:count-people" })).await.unwrap();
    assert!(!lines(&entity["team_standards"]).iter().any(|l| l.starts_with("standard:count-people")), "{entity}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn the_core_naming_standard_is_team_wide_until_refined() {
    // concept:any-model is the core pack's "every model".
    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["concept:return"] })).await.unwrap();
    assert!(ok, "{context}");
    assert!(lines(&context["team_standards"]).iter().any(|l| l.starts_with("standard:model-naming")), "{context}");
    session.close().await.unwrap();
}
