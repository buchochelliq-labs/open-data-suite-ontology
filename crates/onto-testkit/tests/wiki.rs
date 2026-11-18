//! Wiki sources (#49): articles are background about the team's entities,
//! never the team's agreed facts.

use onto_testkit::{Fixture, Session};
use onto_workspace::digest::{DigestOptions, render};
use serde_json::json;

const MRR_HISTORY: &str = "wiki:notes/metrics/mrr-history";
const CHURN_NOTES: &str = "wiki:notes/metrics/churn-notes";

#[tokio::test]
async fn an_entity_lists_wiki_articles_about_it_as_background() {
    let fixture = Fixture::new("wiki").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    let (ok, mrr) = session.call("ontology_entity", json!({ "id": "concept:mrr" })).await.unwrap();
    assert!(ok, "{mrr}");
    // The team's definition is untouched: an article's claims are about the article.
    let facts = mrr["facts"].to_string();
    assert!(facts.contains("excluding one-off charges. [repo reviewed]"), "{mrr}");
    assert!(!facts.contains("notes"), "no wiki fact is presented as the concept's own: {mrr}");
    let background = mrr["background"].as_array().unwrap();
    assert_eq!(background.len(), 1, "{mrr}");
    let line = background[0].as_str().unwrap();
    assert!(line.starts_with(MRR_HISTORY), "{line}");
    assert!(line.contains("\"How we arrived at our MRR definition\"") && line.contains("exclude one-off setup fees"), "{line}");
    assert!(line.contains("unreviewed wiki background") && line.contains("[notes inferred]"), "{line}");

    // An article marks itself as background and keeps its own sources.
    let (_, article) = session.call("ontology_entity", json!({ "id": MRR_HISTORY })).await.unwrap();
    let facts = article["facts"].to_string();
    assert!(facts.contains("background: unreviewed wiki article"), "{article}");
    assert!(facts.contains("cites: [\\\"Finance sync, 2024-03-12\\\"]"), "{article}");
    assert!(facts.contains(&format!("links_to → {CHURN_NOTES}")), "{article}");
    assert!(facts.contains("about → concept:mrr"), "{article}");

    // Instruction-like text in an article is marked wherever it is listed.
    let (_, churn) = session.call("ontology_entity", json!({ "id": "concept:churn" })).await.unwrap();
    let line = churn["background"][0].as_str().unwrap();
    assert!(line.starts_with(CHURN_NOTES) && line.contains("INSTRUCTION-LIKE TEXT"), "{churn}");
    let (_, notes) = session.call("ontology_entity", json!({ "id": CHURN_NOTES })).await.unwrap();
    let facts = notes["facts"].to_string();
    assert!(facts.contains("missing_provenance: the article lists no sources"), "{notes}");
    assert!(facts.contains("instruction_like_text") && facts.contains("INSTRUCTION-LIKE TEXT"), "{notes}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn context_and_search_reach_articles() {
    let fixture = Fixture::new("wiki").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    let (ok, context) = session.call("ontology_context", json!({ "ids": ["concept:mrr"] })).await.unwrap();
    assert!(ok, "{context}");
    assert!(context["relationships"].to_string().contains(&format!("{MRR_HISTORY} about → concept:mrr [notes inferred]")), "{context}");
    assert!(context["entities"].as_array().unwrap().iter().any(|e| e["id"] == MRR_HISTORY), "{context}");

    // Words only an article uses find it, marked as background.
    let (_, search) = session.call("ontology_search", json!({ "query": "setup fees" })).await.unwrap();
    let hit = &search["hits"][0];
    assert_eq!(hit["id"], MRR_HISTORY, "{search}");
    assert!(hit["summary"].as_str().unwrap().contains("unreviewed wiki background"), "{search}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_wiki_statement_becomes_a_fact_only_through_review() {
    let fixture = Fixture::new("wiki").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, proposal) = session
        .call(
            "ontology_propose",
            json!({
                "subject": "concept:mrr",
                "predicate": "history",
                "object": { "literal": "Setup fees were excluded from MRR in March 2024." },
                "rationale": format!("From {MRR_HISTORY}; the user confirmed it.")
            }),
        )
        .await
        .unwrap();
    assert!(ok, "{proposal}");
    let (_, before) = session.call("ontology_entity", json!({ "id": "concept:mrr" })).await.unwrap();
    assert!(before["facts"].to_string().contains("PENDING proposal, not a fact"), "{before}");

    session.human_accepts(proposal["digest"].as_str().unwrap(), "data-team-lead").unwrap();
    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    workspace.sync(&mut store, &Default::default()).unwrap();
    drop(store);
    let mut session = Session::start(&fixture).await.unwrap();
    let (_, after) = session.call("ontology_entity", json!({ "id": "concept:mrr" })).await.unwrap();
    assert!(after["facts"].to_string().contains("history: Setup fees were excluded from MRR in March 2024. [repo reviewed by data-team-lead]"), "{after}");
    session.close().await.unwrap();
}

#[test]
fn the_digest_leaves_articles_out() {
    let fixture = Fixture::new("wiki").unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let text = render(&workspace, &store, &DigestOptions::default()).unwrap();
    assert!(text.contains("concept:mrr"), "{text}");
    assert!(!text.contains("wiki:notes") && !text.contains("setup fees"), "{text}");
}

/// The wiki eval tasks (`evals/saas-wiki-*`) can only pass from the wiki: what
/// they need is in its articles, reachable from the concept, and nowhere else.
#[tokio::test]
async fn the_wiki_eval_overlay_puts_its_answer_only_in_the_wiki() {
    if !onto_testkit::fixture::ods_available() {
        return;
    }
    let overlay = onto_testkit::fixture::repo_root().join("evals/overlays/saas-team-wiki");
    let fixture = Fixture::with_overlay("saas", Some(&overlay)).unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let (ok, mrr) = session.call("ontology_entity", json!({ "id": "concept:mrr" })).await.unwrap();
    assert!(ok, "{mrr}");
    let background = mrr["background"].to_string();
    assert!(background.contains("wiki:wiki/metrics/annual-plans-in-mrr") && background.contains("38%"), "{mrr}");
    assert!(background.contains("wiki:wiki/archive/mrr-2024-definition") && background.contains("setup fees"), "{mrr}");
    // The agreed definition is untouched and still excludes one-off charges.
    assert!(mrr["facts"].to_string().contains("one-off charges") && !mrr["facts"].to_string().contains("38%"), "{mrr}");
    session.close().await.unwrap();

    // Neither the team's pack, the dbt project nor the digest has the story.
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let digest = render(&workspace, &store, &DigestOptions::default()).unwrap();
    // `knows_why` in saas-wiki-annual-plans-history matches "38%": it must not be here.
    assert!(!digest.contains("38%") && !digest.contains("board deck counted"), "{digest}");
    for file in ["fixtures/saas/target/manifest.json", "fixtures/saas/.ontology/concepts.yaml"] {
        let text = std::fs::read_to_string(onto_testkit::fixture::repo_root().join(file)).unwrap();
        assert!(!text.contains("38%"), "{file}");
    }
}
