//! `onto wiki build` (#49): the ontology as a browsable wiki.

use onto_testkit::Fixture;
use onto_testkit::fixture::ods_available;
use onto_workspace::wikibuild::{build, write};

fn pages(name: &str) -> std::collections::BTreeMap<String, String> {
    let fixture = Fixture::new(name).unwrap();
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let files = build(&workspace, &store).unwrap();
    // The same store gives the same bytes.
    assert_eq!(files, build(&workspace, &store).unwrap());
    // To look at the pages: ONTO_WIKI_BUILD_OUT=<dir> cargo test --test wiki_build
    if let Ok(dir) = std::env::var("ONTO_WIKI_BUILD_OUT") {
        write(&std::path::Path::new(&dir).join(name), &files, false).unwrap();
    }
    files
}

#[test]
fn a_concept_page_separates_agreed_facts_background_and_links() {
    let files = pages("wiki");
    let mrr = &files["concepts/concept-mrr.md"];
    assert!(mrr.starts_with("---\ntitle: \"Monthly recurring revenue\"\nid: \"concept:mrr\""), "{mrr}");
    assert!(mrr.contains("generated_by: \"onto wiki build\""), "{mrr}");
    assert!(mrr.contains("excluding one-off charges. _(repo, reviewed · `concepts.yaml#concept:mrr`)_"), "every fact cites its file: {mrr}");
    let background = mrr.split("## Background").nth(1).expect("a background section");
    assert!(background.contains("How we arrived at our MRR definition") && background.contains("_(notes, inferred · `metrics/mrr-history.md`)_"), "{mrr}");
    assert!(!mrr.split("## Background").next().unwrap().contains("setup fees"), "background stays in its section: {mrr}");

    let churn = &files["concepts/concept-churn.md"];
    assert!(churn.contains("INSTRUCTION-LIKE TEXT") && churn.contains("(cites no sources)"), "{churn}");
    // Articles are the wiki's own pages, not pages here.
    assert!(files.keys().all(|p| !p.contains("wiki-notes")), "{:?}", files.keys());
    assert!(files["concepts/_index.md"].contains("| [Churn](concept-churn.md) | `concept:churn` |"), "{}", files["concepts/_index.md"]);
    assert!(files["README.md"].contains("- [Concepts](concepts/_index.md): 3"), "{}", files["README.md"]);
    // A relationship that has ended is not listed as current on either side.
    assert!(!mrr.contains("Legacy MRR") && !files["concepts/concept-legacy_mrr.md"].contains("replaced_by"), "{mrr}");
    // A column has no page of its own, so every side of its disagreement is on the model's page.
    let model = &files["models/model.demo.subscriptions.md"];
    assert!(model.contains("**unit** (sources disagree, **unresolved**: the owners must agree):"), "{model}");
    assert!(model.contains("USD cents _(repo, reviewed · `concepts.yaml#model.demo.subscriptions#amount`)_"), "{model}");
    assert!(model.contains("USD _(repo, reviewed · `finance.yaml#model.demo.subscriptions#amount`)_"), "{model}");
}

#[test]
fn model_pages_show_observed_facts_conflicts_and_known_issues() {
    if !ods_available() {
        return;
    }
    let files = pages("saas");
    let mrr = files.get("models/model.acme.fct_mrr.md").unwrap_or_else(|| panic!("{:?}", files.keys()));
    for section in ["## Open known issues", "## Agreed", "## Observed", "## Columns"] {
        assert!(mrr.contains(section), "{section}: {mrr}");
    }
    assert!(mrr.contains("[issue:DATA-412") || mrr.contains("../known-issues/issue-DATA-412.md"), "{mrr}");
    // Relationships link to the other page.
    assert!(mrr.contains("](../concepts/concept-mrr.md)"), "{mrr}");
    assert!(files.contains_key("known-issues/issue-DATA-412.md"), "{:?}", files.keys());
    assert!(files.contains_key("standards/standard-money-columns.md"), "{:?}", files.keys());
    // An unresolved conflict is listed on the home page and shown on its page with every side.
    let home = &files["README.md"];
    assert!(home.contains("## Unresolved conflicts") && home.contains("concepts/concept-arr.md"), "{home}");
    let arr = &files["concepts/concept-arr.md"];
    assert!(arr.contains("## Sources disagree") && arr.contains("**unresolved**"), "{arr}");
}
