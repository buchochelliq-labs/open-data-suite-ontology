//! ontology_path (#14): the shortest paths between two entities.

use onto_testkit::fixture::ods_available;
use onto_testkit::{Fixture, Session};
use serde_json::{Value, json};

async fn path(session: &mut Session, args: Value) -> Value {
    let (ok, result) = session.call("ontology_path", args).await.unwrap();
    assert!(ok, "{result}");
    result
}

fn steps(result: &Value) -> Vec<Vec<String>> {
    result["paths"].as_array().unwrap().iter().map(|p| p["steps"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()).collect()
}

#[tokio::test]
async fn finds_every_shortest_path_through_a_cycle_and_skips_ended_relationships() {
    let fixture = Fixture::new("paths").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    // The 2020 `billed_to` shortcut has ended; the cycle back from account is walked once.
    let result = path(&mut session, json!({ "from": "concept:order", "to": "concept:invoice" })).await;
    assert_eq!(result["found"], true, "{result}");
    let paths = steps(&result);
    assert_eq!(paths.len(), 2, "two routes of three steps: {result}");
    assert!(result["paths"].as_array().unwrap().iter().all(|p| p["hops"] == 3), "{result}");
    assert!(!result.to_string().contains("billed_to"), "{result}");
    // A step walked backwards still reads as its source states it, with its citation.
    assert_eq!(paths[0][0], "concept:account has_order → concept:order [repo reviewed]", "{result}");
    let ends: Vec<&str> = paths.iter().map(|p| p[2].as_str()).collect();
    assert!(ends.contains(&"concept:contract invoiced_as → concept:invoice [repo reviewed]"), "{result}");
    assert!(ends.contains(&"concept:subscription invoiced_as → concept:invoice [repo reviewed]"), "{result}");

    // Qualifiers are part of a step.
    let result = path(&mut session, json!({ "from": "concept:customer", "to": "concept:order" })).await;
    assert_eq!(steps(&result), vec![vec!["concept:order placed_by → concept:customer (cardinality: many_to_one) [repo reviewed]".to_string()]], "{result}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn respects_the_hop_limit_and_says_why_nothing_was_found() {
    let fixture = Fixture::new("paths").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    // concept:c1 is four steps from order: found by default, not within three.
    let found = path(&mut session, json!({ "from": "concept:order", "to": "concept:c1" })).await;
    assert_eq!(found["paths"][0]["hops"], 4, "{found}");
    let limited = path(&mut session, json!({ "from": "concept:order", "to": "concept:c1", "max_hops": 3 })).await;
    assert_eq!(limited["found"], false, "{limited}");
    assert!(limited["note"].as_str().unwrap().contains("no path within 3 relationships"), "{limited}");

    // Nine steps: beyond even the largest limit.
    let far = path(&mut session, json!({ "from": "concept:order", "to": "concept:c6", "max_hops": 99 })).await;
    assert!(far["note"].as_str().unwrap().contains("no path within 6 relationships"), "{far}");

    // A known issue affecting both does not connect them.
    let island = path(&mut session, json!({ "from": "concept:island", "to": "concept:order" })).await;
    assert_eq!(island["found"], false, "{island}");

    let unknown = path(&mut session, json!({ "from": "concept:ordr", "to": "concept:invoice" })).await;
    assert_eq!(unknown["found"], false);
    assert!(unknown["not_found"][0].as_str().unwrap().starts_with("concept:ordr"), "{unknown}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn labels_dbt_structure_and_follows_team_facts() {
    if !ods_available() {
        return;
    }
    let fixture = Fixture::new("jaffle").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    // Two hops of team facts: the model represents the concept, which net sales measures.
    let result = path(&mut session, json!({ "from": "model.jaffle_ods.orders", "to": "concept:net_sales" })).await;
    assert_eq!(
        steps(&result)[0],
        vec!["model.jaffle_ods.orders represents → concept:order [repo reviewed]", "concept:order measured_by → concept:net_sales [repo reviewed]"],
        "{result}"
    );
    assert!(result.get("note").is_none(), "{result}");

    // A key between two models comes from OpenDataSuite, and is labelled as structure, not lineage.
    let result = path(&mut session, json!({ "from": "model.jaffle_ods.orders", "to": "model.jaffle_ods.customers" })).await;
    let text = result.to_string();
    assert!(text.contains("(dbt structure)") && text.contains("not lineage"), "{result}");
    session.close().await.unwrap();
}

/// The eval task `saas-board-metrics-to-accounts` asks how two models relate;
/// the team's facts connect them through the MRR concept.
#[tokio::test]
async fn the_pilot_connects_board_metrics_to_accounts_through_mrr() {
    if !ods_available() {
        return;
    }
    let fixture = Fixture::new("saas").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let result = path(&mut session, json!({ "from": "model.acme.rpt_board_metrics", "to": "model.acme.dim_accounts" })).await;
    let text = result.to_string();
    assert!(
        text.contains("model.acme.rpt_board_metrics measures → concept:mrr") && text.contains("model.acme.dim_accounts represents → concept:account"),
        "{result}"
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn steps_use_only_current_usable_values_and_their_current_sources() {
    let fixture = Fixture::new("paths").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();

    // central's owner lost to repo's on a single-valued predicate: not a connection.
    let rejected = path(&mut session, json!({ "from": "team:sales", "to": "concept:invoice", "max_hops": 1 })).await;
    assert_eq!(rejected["found"], false, "{rejected}");
    // team:finance has no facts of its own, only relationships pointing to it: still reachable.
    let kept = path(&mut session, json!({ "from": "team:finance", "to": "concept:invoice", "max_hops": 1 })).await;
    assert_eq!(steps(&kept)[0], vec!["concept:invoice primary_owner → team:finance [repo reviewed]"], "{kept}");

    // An unresolved disagreement is a step only as a disputed one.
    let disputed = path(&mut session, json!({ "from": "concept:refund", "to": "concept:payment" })).await;
    let step = &steps(&disputed)[0][0];
    assert!(step.starts_with("concept:refund reverses → concept:payment") && step.contains("DISPUTED"), "{disputed}");
    assert!(disputed["note"].as_str().unwrap().contains("show both and ask"), "{disputed}");

    // repo's statement ended in 2020; only central, which still asserts it, is cited.
    let renewal = path(&mut session, json!({ "from": "concept:renewal", "to": "concept:contract" })).await;
    assert_eq!(steps(&renewal)[0], vec!["concept:renewal renews → concept:contract [central declared]"], "{renewal}");
    session.close().await.unwrap();
}
