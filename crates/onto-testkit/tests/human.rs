//! A second model plays the person: it sees what a person would see, follows
//! its persona, accepts proposals when it agrees, and ends the conversation.

#![cfg(feature = "llm")]

use std::time::Duration;

use onto_testkit::agent::{ModelConfig, Reply, last_proposal};
use onto_testkit::eval::{Agent, Cast, Condition, Task, run_task};
use onto_testkit::fake_model::{say, serve, tool_call};
use onto_testkit::fixture::repo_root;
use onto_testkit::human::{parse_reply, person_view};
use onto_testkit::{Event, Transcript};
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
fn a_proposal_is_accepted_once() {
    let propose = |digest: &str| Event::ToolCall { tool: "ontology_propose".into(), arguments: json!({}), ok: true, result: json!({ "digest": digest }) };
    let accept = |digest: &str| Event::HumanAction { action: "accept".into(), detail: json!({ "digest": digest, "by": "p" }) };
    let mut transcript = Transcript { events: vec![propose("a"), propose("b")] };
    assert_eq!(last_proposal(&transcript).as_deref(), Some("b"));
    // A simulated person said ACCEPT twice in a pilot run. The second must
    // neither accept "b" again (which aborted the run) nor fall back to "a".
    transcript.events.push(accept("b"));
    assert_eq!(last_proposal(&transcript), None);
}

#[test]
fn replies_are_parsed_into_actions() {
    assert!(matches!(parse_reply("DONE", true, "p"), Reply::End));
    assert!(matches!(parse_reply("  done. ", true, "p"), Reply::End));
    assert!(matches!(parse_reply("", true, "p"), Reply::End));
    match parse_reply("ACCEPT\nThanks, that's right.", true, "p") {
        Reply::AcceptLastProposal { by, then } => {
            assert_eq!(by, "p");
            assert_eq!(then.as_deref(), Some("Thanks, that's right."));
        }
        other => panic!("{other:?}"),
    }
    // A persona that may not accept can only talk.
    assert!(matches!(parse_reply("ACCEPT\nok", false, "p"), Reply::Say(_)));
    assert!(matches!(parse_reply("Done with the first part, now the key?", true, "p"), Reply::Say(_)));
}

#[test]
fn the_person_sees_messages_but_not_tool_calls() {
    let transcript = Transcript {
        events: vec![
            Event::Human { text: "What is a customer?".into() },
            Event::ToolCall { tool: "ontology_entity".into(), arguments: json!({ "id": "concept:customer" }), ok: true, result: json!({ "secret": 1 }) },
            Event::Agent { text: "An account holder.".into() },
        ],
    };
    let view = person_view(&transcript);
    assert!(view.contains("YOU: What is a customer?") && view.contains("ASSISTANT: An account holder."));
    assert!(!view.contains("ontology_entity") && !view.contains("secret"));
}

#[tokio::test]
async fn one_model_plays_the_agent_and_another_the_person() {
    let tasks = Task::load_dir(&repo_root().join("evals")).unwrap();
    let task = tasks.iter().find(|t| t.name == "customer-grain-proposal").unwrap();
    assert!(task.persona.as_ref().is_some_and(|p| p.accepts_proposals));

    let (agent_url, _) = serve(vec![
        tool_call("1", "ontology_entity", json!({ "id": "concept:customer" })),
        say("Sources disagree: the central pack says a buyer, the repository says an account holder. Which should the dimension use?"),
        tool_call("2", "ontology_propose", json!({ "subject": "concept:customer", "predicate": "grain", "object": { "literal": "one row per person" }, "rationale": "The user said so in this session." })),
        say("I proposed grain = one row per person. It is pending until a person accepts it with onto proposals accept."),
        say("Thanks; the grain is now recorded."),
    ])
    .await;
    let (person_url, person_requests) = serve(vec![
        say("Use the repository definition. One row per person, even with several accounts. Please record that."),
        say("ACCEPT\nGreat, thanks."),
        say("DONE"),
    ])
    .await;

    let result =
        run_task(task, Condition::ONTOLOGY_ONLY, 1, Cast { person: Some(&Agent::Api(config(person_url))), ..Cast::new(&Agent::Api(config(agent_url))) })
            .await
            .unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    let events = &result.transcript.events;
    assert!(events.iter().any(|e| matches!(e, Event::HumanAction { action, .. } if action == "accept")), "the person accepted the proposal");
    assert!(events.iter().any(|e| matches!(e, Event::Human { text } if text.contains("Great, thanks."))), "and then said so");
    assert!(result.violations.is_empty(), "{:?}", result.violations);

    let requests = person_requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "the person replied twice, then ended the conversation");
    let system = requests[0]["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("data modeller designing the shop's customer dimension"), "the persona reaches the person model");
    assert!(system.contains("ACCEPT"), "a persona that may accept is told how");
    let prompt = requests[1]["messages"][1]["content"].as_str().unwrap();
    assert!(prompt.contains("ASSISTANT: Sources disagree"), "the person sees the agent's messages");
    assert!(!prompt.contains("ontology_entity"), "but not its tool calls");
}
