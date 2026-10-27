//! The evaluation pipeline end to end with scripted models: tasks load, an
//! episode runs under a condition, criteria score, and the report renders.

#![cfg(feature = "llm")]

use std::time::Duration;

use onto_testkit::agent::ModelConfig;
use onto_testkit::eval::errors;
use onto_testkit::eval::{Agent, Cast, Condition, ODS_GUIDANCE, Outcome, SYSTEM_PROMPT, Task, report, run_task, system_prompt};
use onto_testkit::fake_model::{error, say, serve, tool_call};
use onto_testkit::fixture::repo_root;
use serde_json::json;

fn config(base_url: String) -> ModelConfig {
    ModelConfig {
        base_url,
        model: "fake".into(),
        api_key: None,
        max_turns: 10,
        max_tool_result_chars: 4_000,
        timeout: Duration::from_secs(10),
        requests_per_minute: None,
        budget: None,
    }
}

#[test]
fn every_task_file_parses_and_has_criteria() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    assert!(tasks.len() >= 4);
    for task in &tasks {
        assert!(!task.criteria.is_empty(), "{}", task.name);
        assert!(repo_root().join("fixtures").join(&task.fixture).is_dir(), "{} uses unknown fixture {}", task.name, task.fixture);
    }
}

#[test]
fn each_condition_gets_matching_grounding_guidance() {
    let baseline = system_prompt(Condition::ODS_ONLY, None);
    assert!(baseline.starts_with(SYSTEM_PROMPT) && baseline.contains(ODS_GUIDANCE), "the baseline gets equivalent ODS guidance");
    assert!(!baseline.contains("ontology_"), "the baseline names no ontology tools");
    for condition in [Condition::ONTOLOGY_ONLY, Condition::BOTH] {
        let prompt = system_prompt(condition, None);
        assert!(prompt.starts_with(SYSTEM_PROMPT));
        assert!(prompt.contains("# Ground a design in the ontology"), "{condition:?}");
        assert!(!prompt.contains("name: ontology-grounding"), "front matter is stripped");
    }
}

#[tokio::test]
async fn a_task_runs_scores_and_reports() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();
    assert!(task.applies_to(Condition::ONTOLOGY_ONLY) && !task.applies_to(Condition::ODS_ONLY));

    let (agent_url, requests) = serve(vec![
        tool_call("1", "ontology_entity", json!({ "id": "concept:customer" })),
        // A tool from a hidden server is refused, not routed.
        tool_call("2", "ods_describe_entity", json!({ "entity": "customers" })),
        say("Two sources disagree: the central pack says a buyer, the repository says an account holder with a completed order."),
        tool_call("3", "ontology_propose", json!({ "subject": "concept:customer", "predicate": "grain", "object": { "literal": "one row per person" }, "rationale": "User decision in this session." })),
        say("I proposed the grain; it is pending review until someone runs onto proposals accept."),
        say("You're welcome."),
    ])
    .await;
    let (judge_url, _) = serve(vec![say(r#"Sure: {"pass": true, "reason": "It says the proposal is pending review."}"#)]).await;

    let result = run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast { judge: Some(&Agent::Api(config(judge_url))), ..Cast::new(&Agent::Api(config(agent_url))) })
        .await
        .unwrap();
    let outcome = |id: &str| result.criteria.iter().find(|c| c.id == id).unwrap().outcome;
    assert_eq!(outcome("surfaces_conflict"), Outcome::Pass);
    assert_eq!(outcome("proposes_grain"), Outcome::Pass);
    assert_eq!(outcome("honest_about_review"), Outcome::Pass);
    assert_eq!(outcome("grounded"), Outcome::Pass);
    assert_eq!(result.failed_tool_calls, 1, "the hidden ODS tool was refused");
    assert!(result.violations.is_empty());

    let tools: Vec<String> =
        requests.lock().unwrap()[0]["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
    assert!(tools.iter().all(|t| t.starts_with("ontology_")), "only the condition's tools are offered: {tools:?}");

    let markdown = report(&[result]);
    assert!(markdown.contains("| customer-grain-proposal | proposes_grain | 1/1 |"), "{markdown}");
    assert!(markdown.contains("Invariant violations: 0"));
}

#[tokio::test]
async fn judge_criteria_are_skipped_without_a_judge() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();
    let (agent_url, _) = serve(vec![say("I don't know.")]).await;
    let result = run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast::new(&Agent::Api(config(agent_url)))).await.unwrap();
    let judged = result.criteria.iter().find(|c| c.id == "honest_about_review").unwrap();
    assert_eq!(judged.outcome, Outcome::Skipped);
    assert_eq!(result.criteria.iter().find(|c| c.id == "grounded").unwrap().outcome, Outcome::Fail);
}

#[tokio::test]
async fn a_failing_provider_is_an_error_not_a_score() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();

    // The agent's provider rejects the request (e.g. retired or misconfigured).
    let (agent_url, _) = serve(vec![error(404, None)]).await;
    let (judge_url, _) = serve(vec![error(404, None)]).await;
    let result = run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast { judge: Some(&Agent::Api(config(judge_url))), ..Cast::new(&Agent::Api(config(agent_url))) })
        .await
        .unwrap();
    assert!(result.error.as_deref().unwrap().contains("404"), "{:?}", result.error);
    assert_eq!(result.criteria.iter().find(|c| c.id == "honest_about_review").unwrap().outcome, Outcome::Error);

    let results = [result];
    assert_eq!(errors(&results).len(), 2, "the episode and the judged criterion");
    assert!(report(&results).contains("not scores"));
}

#[tokio::test]
async fn an_exhausted_quota_marks_the_run_so_the_runner_stops() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();
    let quota = json!({ "error": { "code": 429, "details": [{ "quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier" }] } });
    let (agent_url, _) = serve(vec![onto_testkit::fake_model::error_with_body(429, quota)]).await;
    let result = run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast::new(&Agent::Api(config(agent_url)))).await.unwrap();
    assert!(result.budget_exhausted);
    assert!(result.error.as_deref().unwrap().contains("daily quota"));
}

#[tokio::test]
async fn conversations_render_as_a_readable_chat() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();
    let (agent_url, _) = serve(vec![tool_call("1", "ontology_entity", json!({ "id": "concept:customer" })), say("A customer is an account holder.")]).await;
    let result = run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast::new(&Agent::Api(config(agent_url)))).await.unwrap();
    let chat = onto_testkit::eval::render_chat("customer / ontology_only / run 1", &result);
    assert!(chat.contains("**🧑 Person:** I'm designing the customer dimension"), "{chat}");
    assert!(chat.contains("🔧 **ontology_entity**"));
    assert!(chat.contains("\"id\": \"concept:customer\""), "arguments are shown in full");
    assert!(chat.contains("Finance only counts customers"), "tool results are shown in full, not clipped");
    assert!(chat.contains("**🤖 Agent:** A customer is an account holder."));
    assert!(chat.contains("| grounded | Pass |"));
}

#[test]
fn conditions_are_named_by_tools_and_context() {
    for name in ["ods_only", "ontology_only", "both", "both+digest", "ods_only+digest"] {
        assert_eq!(name.parse::<Condition>().unwrap().name(), name);
    }
    assert!("everything".parse::<Condition>().is_err());
    let with_digest: Condition = "ods_only+digest".parse().unwrap();
    assert!(with_digest.digest && with_digest.tools == Condition::ODS_ONLY.tools);
    // A task limited to a tool set also runs with the digest added.
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();
    assert!(task.applies_to("ontology_only+digest".parse().unwrap()) && !task.applies_to("both+digest".parse().unwrap()));
}

#[test]
fn the_digest_is_added_only_for_digest_conditions() {
    let digest = "# Team context\n- **Net sales**";
    assert!(!system_prompt(Condition::BOTH, Some(digest)).contains("Net sales"));
    let prompt = system_prompt("both+digest".parse().unwrap(), Some(digest));
    assert!(prompt.contains("generated from the ontology with `onto context`") && prompt.ends_with(digest));
    assert!(system_prompt("ods_only+digest".parse().unwrap(), Some(digest)).contains(ODS_GUIDANCE));
}

#[test]
fn new_tasks_need_the_ontology_by_construction() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    for name in ["revenue-measure-multi-hop", "revenue-definition-paraphrase", "naming-standard", "order-history-unnamed"] {
        let task = tasks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} is missing"));
        assert!(task.persona.is_some(), "{name} has a persona for simulated runs");
    }
    // The paraphrase task's key word appears nowhere the agent could match it.
    let pack = std::fs::read_to_string(repo_root().join("fixtures/jaffle/.ontology/concepts.yaml")).unwrap();
    let manifest = std::fs::read_to_string(repo_root().join("fixtures/jaffle/target/manifest.json")).unwrap();
    assert!(!pack.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| l.to_lowercase().contains("revenue")));
    assert!(!manifest.to_lowercase().contains("revenue"));
}
