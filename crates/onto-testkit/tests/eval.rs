//! The evaluation pipeline end to end with scripted models: tasks load, an
//! episode runs under a condition, criteria score, and the report renders.

#![cfg(feature = "llm")]

use std::time::Duration;

use onto_testkit::Event;
use onto_testkit::agent::ModelConfig;
use onto_testkit::eval::errors;
use onto_testkit::eval::{Agent, Cast, Condition, Exploration, ODS_GUIDANCE, Outcome, SYSTEM_PROMPT, Task, report, run_task, system_prompt};
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
    assert!(baseline.contains("# Steno: brief while working"), "every condition gets the same working style");
    for condition in [Condition::ONTOLOGY_ONLY, Condition::BOTH] {
        let prompt = system_prompt(condition, None);
        assert!(prompt.starts_with(SYSTEM_PROMPT));
        assert!(prompt.contains("# Ground a design in the ontology"), "{condition:?}");
        assert!(prompt.contains("# Steno: brief while working, clear when answering"), "{condition:?}");
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
    assert!(chat.contains("An account holder with at least one completed order."), "tool results are shown in full, not clipped");
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
    // The expected staging name follows only the team's rule, not the project or dbt's default.
    assert!(!manifest.contains("src_") && manifest.contains("stg_"));
}

#[test]
fn characters_read_count_what_the_agent_received() {
    use onto_testkit::{Event, Transcript};
    let transcript = Transcript {
        events: vec![
            Event::ToolCall { tool: "ontology_entity".into(), arguments: json!({ "id": "a" }), ok: true, result: json!("x".repeat(10_000)) },
            Event::ToolCall { tool: "ontology_search".into(), arguments: json!({ "query": "revenue" }), ok: true, result: json!({ "hits": [] }) },
        ],
    };
    let capped = Exploration::of(&transcript, Some(4_000));
    assert_eq!(capped.chars_read, 4_000 + json!({ "hits": [] }).to_string().len(), "a truncated result counts as delivered");
    assert_eq!(Exploration::of(&transcript, None).chars_read, 10_000 + 11);
    assert_eq!(capped.entities, 1);
    assert_eq!(capped.empty_searches, vec!["ontology_search: revenue".to_string()]);
}

#[tokio::test]
async fn every_judged_question_goes_in_one_request() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("two-questions.yaml"),
        r#"
name: two-questions
fixture: shop
conditions: [ontology_only]
prompt: What is a customer?
criteria:
  - { id: grounded, judge: "Is the answer grounded?" }
  - { id: concise, judge: "Is the answer concise?" }
  - { id: missing, judge: "A question the judge forgets." }
"#,
    )
    .unwrap();
    let tasks = Task::load_dir(dir.path()).unwrap();
    let (agent_url, _) = serve(vec![say("An account holder.")]).await;
    let (judge_url, judge_requests) =
        serve(vec![say(r#"{"grounded": {"pass": true, "reason": "Cites a source."}, "concise": {"pass": false, "reason": "Too long."}}"#)]).await;
    let (agent, judge) = (Agent::Api(config(agent_url)), Agent::Api(config(judge_url)));
    let cast = Cast { judge: Some(&judge), ..Cast::new(&agent) };
    let result = run_task(&tasks[0], Condition::ONTOLOGY_ONLY, 1, cast).await.unwrap();
    let outcome = |id: &str| result.criteria.iter().find(|c| c.id == id).unwrap().outcome;
    assert_eq!((outcome("grounded"), outcome("concise"), outcome("missing")), (Outcome::Pass, Outcome::Fail, Outcome::Error));
    let requests = judge_requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "one judge request for three questions");
    let prompt = requests[0]["messages"][1]["content"].as_str().unwrap();
    assert!(prompt.contains("- grounded: Is the answer grounded?") && prompt.contains("- concise: Is the answer concise?"));
}

#[tokio::test]
async fn a_fact_accepted_in_one_session_is_used_in_the_next() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "snapshot-grain-carries-over").unwrap();
    let rank = "model.jaffle_ods.customers_snapshot_view";
    // One scripted model plays the agent in both sessions, in order.
    let (agent_url, _) = serve(vec![
        // Session 1: propose, then wait for the person to accept.
        tool_call("1", "ontology_propose", json!({ "subject": rank, "predicate": "grain", "object": { "literal": "one row per customer per snapshot day" }, "rationale": "The model's owner stated it in this session." })),
        say("I proposed grain = one row per customer per snapshot day. It is pending until you accept it with onto proposals accept."),
        say("It is still pending your review."),
        say("Thanks, it is accepted."),
        say("You're welcome."),
        // Session 2: a fresh agent looks the model up.
        tool_call("2", "ontology_entity", json!({ "id": rank })),
        say("customers_snapshot_view is one row per customer per snapshot day [repo reviewed by scripted-person]."),
        say("Glad it helps."),
    ])
    .await;
    let agent = Agent::Api(config(agent_url));
    // ontology_only: the fixture's ODS source still syncs, but only ontology tools are visible.
    let result = run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast::new(&agent)).await.unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    let outcome = |id: &str| result.criteria.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no {id}")).outcome;
    assert_eq!(outcome("proposed_grain"), Outcome::Pass);
    assert_eq!(outcome("followup.knows_recorded_grain"), Outcome::Pass);
    assert_eq!(outcome("followup.cites_record"), Outcome::Skipped, "no judge configured");
    assert!(result.violations.is_empty(), "the repository changed only through the accepted proposal: {:?}", result.violations);

    let events = &result.transcript.events;
    let new_session = events.iter().position(|e| matches!(e, Event::HumanAction { action, .. } if action == "new_session")).expect("a session marker");
    assert!(events[..new_session].iter().any(|e| matches!(e, Event::HumanAction { action, .. } if action == "accept")), "accepted in session 1");
    let looked_up = events[new_session..].iter().find_map(|e| match e {
        Event::ToolCall { tool, result, .. } if tool == "ontology_entity" => Some(result.to_string()),
        _ => None,
    });
    assert!(
        looked_up.unwrap().contains("grain: one row per customer per snapshot day [repo reviewed by scripted-person]"),
        "session 2 reads the accepted fact"
    );
}

#[test]
fn repeated_runs_report_their_spread() {
    use onto_testkit::eval::{CriterionResult, RunResult};
    let run = |n: usize, outcomes: &[Outcome]| {
        let task = Task::load_dir(&repo_root().join("evals")).unwrap().into_iter().find(|t| t.name == "order-events-fan-out").unwrap();
        let mut r = RunResult::not_run(&task, Condition::BOTH, n, "");
        r.skipped = None;
        r.criteria = outcomes.iter().enumerate().map(|(i, o)| CriterionResult { id: format!("c{i}"), outcome: *o, detail: String::new() }).collect();
        r
    };
    let one = report(&[run(1, &[Outcome::Pass, Outcome::Pass])]);
    assert!(!one.contains("Spread across runs"), "a single run has no spread");
    let three = report(&[run(1, &[Outcome::Pass, Outcome::Pass]), run(2, &[Outcome::Pass, Outcome::Fail]), run(3, &[Outcome::Fail, Outcome::Fail])]);
    assert!(three.contains("| both | 3 | 50% | 0% | 100% |"), "{three}");

    // A provider outage or an unanswered judgement is not a failing score.
    let mut outage = run(2, &[Outcome::Fail, Outcome::Fail]);
    outage.error = Some("model returned 503".into());
    let errors = report(&[run(1, &[Outcome::Pass, Outcome::Pass]), outage, run(3, &[Outcome::Pass, Outcome::Error])]);
    assert!(errors.contains("| both | 2 | 100% | 100% | 100% |"), "{errors}");
}
