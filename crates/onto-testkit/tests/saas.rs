//! The pilot fixture: a made-up B2B SaaS dbt project (billing, product usage,
//! CRM) whose team facts span finance, product and sales.

use onto_core::Object;
use onto_testkit::fixture::{ods_available, repo_root};
use onto_testkit::{Fixture, Session};
use onto_workspace::digest::{DigestOptions, render};
use onto_workspace::proposals::{ProposalRequest, accept, prepare};
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
    assert!(
        text.contains("## Known issues\n\n- issue:DATA-412 Plan changes show as churn and reactivation (open, since 2026-01-01, ticket DATA-412) affects model.acme.fct_mrr: "),
        "open known issues come first: {text}"
    );
    assert!(text.contains("Workaround: Treat a churn followed by a reactivation"), "{text}");
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

/// The learning loop records facts on columns (`<model>#<column>`). The edge
/// from model to column comes from dbt, so a pilot run's later session asked
/// for the model's context and never saw the accepted rule.
#[tokio::test]
async fn team_facts_on_a_column_come_with_its_model() {
    if !ods_available() {
        return;
    }
    let fixture = Fixture::new("saas").unwrap();
    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    let request = ProposalRequest {
        subject: "model.acme.customer_health_scores#health_score".into(),
        predicate: "business_rule".into(),
        object: Object::Literal("An account is at risk when health_score is below 40.".into()),
        qualifiers: Default::default(),
        valid: Default::default(),
        rationale: "Stated by the model's owner.".into(),
    };
    let digest = prepare(&workspace, &store, request, None).unwrap().digest;
    accept(&workspace, &mut store, &digest, "owner").unwrap();
    drop(store);

    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.acme.customer_health_scores"] })).await.unwrap();
    assert!(ok, "{context}");
    let text = context.to_string();
    assert!(text.contains("below 40"), "{text}");
    assert!(text.contains("has_column → model.acme.customer_health_scores#health_score"), "the way to it is shown: {text}");
    // Columns without team facts stay with ODS.
    assert!(!text.contains("customer_health_scores#week_start"), "{text}");
    session.close().await.unwrap();
}

fn data412_fixed() -> Fixture {
    Fixture::with_overlay("saas", Some(&repo_root().join("evals/overlays/saas-data412-fixed"))).unwrap()
}

/// An open known issue is listed with the entities it affects, under its own
/// heading, not as a relationship to follow.
#[tokio::test]
async fn an_open_known_issue_comes_with_the_model_it_affects() {
    if !ods_available() {
        return;
    }
    let fixture = Fixture::new("saas").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.acme.fct_mrr"] })).await.unwrap();
    assert!(ok, "{context}");
    let issues = context["known_issues"].to_string();
    assert!(issues.contains("issue:DATA-412") && issues.contains("(open, since 2026-01-01, ticket DATA-412)"), "{context}");
    assert!(!context["relationships"].to_string().contains("affects"), "{context}");
    // Reached through a concept, too.
    let (_, context) = session.call("ontology_context", json!({ "ids": ["concept:mrr_movement"], "depth": 2 })).await.unwrap();
    assert!(context["known_issues"].to_string().contains("issue:DATA-412"), "{context}");
    let (_, entity) = session.call("ontology_entity", json!({ "id": "model.acme.fct_mrr" })).await.unwrap();
    assert!(entity["known_issues"].to_string().contains("issue:DATA-412"), "{entity}");
    session.close().await.unwrap();
}

/// Once fixed, the issue drops out of current answers, so agents stop warning
/// about it, and stays available for the months it affected.
#[tokio::test]
async fn a_fixed_known_issue_is_history_not_a_warning() {
    if !ods_available() {
        return;
    }
    let fixture = data412_fixed();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let text = render(&workspace, &store, &DigestOptions::default()).unwrap();
    assert!(!text.contains("DATA-412") && !text.contains("## Known issues"), "{text}");
    let all = render(&workspace, &store, &DigestOptions { fixed_issues: true, ..Default::default() }).unwrap();
    assert!(
        all.contains("issue:DATA-412 Plan changes show as churn and reactivation (fixed, since 2026-01-01, fixed in 2026-06-01, ticket DATA-412)"),
        "{all}"
    );
    drop(store);

    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, context) = session.call("ontology_context", json!({ "ids": ["model.acme.fct_mrr"], "terms": ["reactivation"], "depth": 2 })).await.unwrap();
    assert!(ok, "{context}");
    assert!(!context.to_string().contains("DATA-412"), "neither listed nor reached by words: {context}");
    let (_, history) = session.call("ontology_context", json!({ "ids": ["model.acme.fct_mrr"], "known_issues": "all" })).await.unwrap();
    assert!(history["known_issues"].to_string().contains("(fixed, since 2026-01-01, fixed in 2026-06-01"), "{history}");

    let (_, now) = session.call("ontology_entity", json!({ "id": "model.acme.fct_mrr" })).await.unwrap();
    assert!(!now.to_string().contains("DATA-412"), "{now}");
    let (_, april) = session.call("ontology_entity", json!({ "id": "model.acme.fct_mrr", "as_of": "2026-04-15" })).await.unwrap();
    assert!(april["known_issues"].to_string().contains("issue:DATA-412"), "it affected April: {april}");
    // Asked for by id, the issue itself says it is fixed.
    let (_, issue) = session.call("ontology_entity", json!({ "id": "issue:DATA-412" })).await.unwrap();
    assert!(issue["facts"].to_string().contains("status: fixed"), "{issue}");
    session.close().await.unwrap();
}
