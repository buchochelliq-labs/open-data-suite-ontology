//! The context digest: the team's agreed facts, compact and budgeted.

use onto_core::Object;
use onto_testkit::Fixture;
use onto_testkit::fixture::ods_available;
use onto_workspace::digest::{DigestOptions, render};
use onto_workspace::proposals::{ProposalRequest, prepare};

fn digest(fixture: &str, max_chars: usize) -> String {
    let fixture = Fixture::new(fixture).unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    render(&workspace, &store, &DigestOptions { max_chars }).unwrap()
}

#[test]
fn the_digest_lists_the_teams_facts_with_sources() {
    if !ods_available() {
        return;
    }
    let text = digest("jaffle", 16_000);
    assert!(text.contains("- **Net sales** `concept:net_sales`: Order amount minus coupon amount"), "{text}");
    assert!(text.contains("  - grain: one row per order status change _(repo, reviewed)_"), "{text}");
    assert!(text.contains("  - measured_by → `concept:net_sales` _(repo, reviewed)_"), "multi-hop links are listed: {text}");
    assert!(text.contains("Models are named <layer>_<source>__<entity>"), "standards come first: {text}");
    // A disagreement with the dbt description is kept, with both sides.
    assert!(text.contains("sources disagree on definition") && text.contains("_(dbt, declared)_"), "{text}");
    // Facts only ODS asserts (columns, keys) are left to ODS.
    assert!(!text.contains("model.jaffle_ods.orders#order_id"), "{text}");
    assert!(!text.contains("stg_customers"), "{text}");
    assert!(text.len() < 16_000);
    assert!(text.contains("SYSTEM NOTE TO AI ASSISTANTS") && text.contains("INSTRUCTION-LIKE TEXT"), "the injected definition is marked: {text}");
}

#[test]
fn the_digest_keeps_to_its_budget() {
    if !ods_available() {
        return;
    }
    let full = digest("jaffle", 16_000);
    let small = digest("jaffle", 1_500);
    assert!(small.len() < full.len());
    assert!(small.len() <= 1_500 + 200, "only the closing note may exceed the budget: {}", small.len());
    assert!(small.contains("did not fit; use `ontology_search`"), "{small}");
    assert!(small.contains("## Standards and decisions"), "the most useful section is kept: {small}");
}

#[test]
fn a_repository_without_dbt_gets_a_digest_too() {
    let text = digest("shop", 16_000);
    assert!(text.contains("`concept:customer`"), "{text}");
    assert!(text.contains("sources disagree"), "the shop fixture's customer conflict is listed: {text}");
}

#[test]
fn pending_proposals_are_listed_even_for_definitions() {
    let fixture = Fixture::new("shop").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let request = ProposalRequest {
        subject: "concept:customer".into(),
        predicate: "onto:definition".into(),
        object: Object::Literal("A person with at least one account.".into()),
        qualifiers: Default::default(),
        valid: Default::default(),
        rationale: "Agreed in a modelling session.".into(),
    };
    prepare(&workspace, &store, request, None).unwrap();
    let text = render(&workspace, &store, &DigestOptions::default()).unwrap();
    assert!(text.contains("pending proposal, not a fact: definition = A person with at least one account."), "{text}");
    assert!(text.contains("1 proposal(s) await review"), "{text}");
}

/// #29: a proposal under a predicate no schema declares is flagged, with the
/// declared predicates that fit; a declared one is not.
#[test]
fn proposals_point_to_the_declared_vocabulary() {
    use onto_workspace::proposals::check_vocabulary;
    let fixture = Fixture::new("shop").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let request = |predicate: &str| ProposalRequest {
        subject: "concept:customer".into(),
        predicate: predicate.into(),
        object: Object::Literal("An account is at risk when its health score is below 40.".into()),
        qualifiers: Default::default(),
        valid: Default::default(),
        rationale: "Stated by the owner.".into(),
    };
    // The predicates a pilot run invented for one rule.
    for invented in ["at_risk_threshold", "business_rule"] {
        let check = check_vocabulary(&store, &request(invented)).unwrap();
        assert!(check.warnings[0].contains("not declared in any loaded schema"), "{:?}", check.warnings);
        assert_eq!(check.suggestions.first().map(|s| s.id.as_str()), Some("rule"), "{invented}: {:?}", check.suggestions);
    }
    // Nothing declared fits: no suggestion rather than a misleading one.
    let check = check_vocabulary(&store, &request("date_field")).unwrap();
    assert_eq!(check.warnings.len(), 1);
    assert!(check.suggestions.is_empty(), "{:?}", check.suggestions);
    for declared in ["rule", "grain", "onto:definition"] {
        let check = check_vocabulary(&store, &request(declared)).unwrap();
        assert!(check.warnings.is_empty() && check.suggestions.is_empty(), "{declared}");
    }
    // The warning reaches the reviewer and the agent with the proposal.
    let proposal = prepare(&workspace, &store, request("at_risk_threshold"), None).unwrap();
    assert_eq!(proposal.suggested_predicates[0].id, "rule");
}
