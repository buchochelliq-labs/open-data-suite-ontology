//! A second model plays the person: it sees what a person would see, follows
//! its persona, accepts proposals when it agrees, and ends the conversation.

#![cfg(feature = "llm")]

use std::time::Duration;

use onto_testkit::agent::{ModelConfig, Reply, last_proposal};
use onto_testkit::eval::{Agent, Cast, Condition, Task, run_task};
use onto_testkit::fake_model::{say, serve, tool_call};
use onto_testkit::fixture::repo_root;
use onto_testkit::human::{DEFAULT_WINDOW, Person, PersonHistory, Persona, SimulatedHuman, parse_reply, person_prompt, person_view, person_view_with};
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

/// A conversation of `n` messages after the opening one: the assistant and
/// the person take turns, with a tool call before each assistant message.
fn conversation(n: usize) -> Transcript {
    let mut events = vec![Event::Human { text: "opening".into() }];
    for i in 1..=n {
        if i % 2 == 1 {
            events.push(Event::ToolCall { tool: "ods_search".into(), arguments: json!({ "query": i }), ok: true, result: json!({}) });
            events.push(Event::Agent { text: format!("a{i}") });
        } else {
            events.push(Event::Human { text: format!("p{i}") });
        }
    }
    Transcript { events }
}

#[test]
fn the_person_history_defaults_to_a_window_and_parses() {
    assert_eq!(DEFAULT_WINDOW, 4);
    assert_eq!(PersonHistory::default(), PersonHistory::Window(4));
    assert_eq!("full".parse::<PersonHistory>().unwrap(), PersonHistory::Full);
    assert_eq!(" 6 ".parse::<PersonHistory>().unwrap(), PersonHistory::Window(6));
    assert!("0".parse::<PersonHistory>().is_err(), "a window must show the latest message");
    assert!("all".parse::<PersonHistory>().is_err());
    for history in [PersonHistory::Full, PersonHistory::Window(3)] {
        assert_eq!(history.to_string().parse::<PersonHistory>().unwrap(), history);
    }
}

#[test]
fn a_conversation_within_the_window_is_shown_whole() {
    let window = PersonHistory::Window(4);
    // Below and exactly at the window: the opening message and every message after it.
    let short = conversation(3);
    assert_eq!(person_view_with(&short, window), "YOU: opening\n\nASSISTANT: a1\n\nYOU: p2\n\nASSISTANT: a3");
    let exact = conversation(4);
    assert_eq!(person_view_with(&exact, window), "YOU: opening\n\nASSISTANT: a1\n\nYOU: p2\n\nASSISTANT: a3\n\nYOU: p4");
    assert_eq!(person_view_with(&exact, window), person_view(&exact));
    assert!(!person_view_with(&exact, window).contains("earlier message"));
}

#[test]
fn earlier_messages_are_summarised_beyond_the_window() {
    let window = PersonHistory::Window(4);
    // One above the window: one message is left out, named in the singular.
    assert_eq!(person_view_with(&conversation(5), window), "YOU: opening\n\n(1 earlier message)\n\nYOU: p2\n\nASSISTANT: a3\n\nYOU: p4\n\nASSISTANT: a5");
    // Well above it: the opening message, the summary, the last four. The
    // whole request the person's model gets, exactly.
    assert_eq!(
        person_prompt(&conversation(9), window),
        "The conversation so far:\n\n\
         YOU: opening\n\n\
         (5 earlier messages)\n\n\
         YOU: p6\n\nASSISTANT: a7\n\nYOU: p8\n\nASSISTANT: a9\n\n\
         Write your next message to the assistant, or DONE."
    );
    // The person's own actions are messages too, and tool calls never are.
    let mut transcript = conversation(5);
    transcript.events.push(Event::HumanAction { action: "accept".into(), detail: json!("d1") });
    let view = person_view_with(&transcript, PersonHistory::Window(2));
    assert_eq!(view, "YOU: opening\n\n(4 earlier messages)\n\nASSISTANT: a5\n\n(you did: accept \"d1\")");
    assert!(!view.contains("ods_search"));
}

#[test]
fn full_history_shows_every_message() {
    let transcript = conversation(9);
    let full = person_view_with(&transcript, PersonHistory::Full);
    assert_eq!(full, person_view(&transcript));
    assert!(!full.contains("earlier message"));
    for i in 1..=9 {
        assert!(full.contains(&format!("{}{i}", if i % 2 == 1 { "a" } else { "p" })), "message {i}");
    }
}

#[tokio::test]
async fn the_simulated_person_is_asked_with_the_windowed_conversation() {
    let persona = Persona {
        role: "analyst".into(),
        goal: "an answer".into(),
        knows: vec!["Finance owns revenue.".into()],
        accepts_proposals: false,
        max_replies: Some(5),
    };
    let transcript = conversation(9);
    let (url, requests) = serve(vec![say("Thanks."), say("DONE")]).await;
    let mut windowed = SimulatedHuman::new(persona.clone(), Agent::Api(config(url.clone())));
    assert!(matches!(windowed.reply(&transcript).await.unwrap(), Reply::Say(text) if text == "Thanks."));
    let mut full = SimulatedHuman::new(persona, Agent::Api(config(url))).with_history(PersonHistory::Full);
    assert!(matches!(full.reply(&transcript).await.unwrap(), Reply::End));

    let requests = requests.lock().unwrap();
    let prompt = |i: usize| requests[i]["messages"][1]["content"].as_str().unwrap().to_string();
    assert_eq!(prompt(0), person_prompt(&transcript, PersonHistory::Window(DEFAULT_WINDOW)), "windowed by default");
    assert!(prompt(0).contains("(5 earlier messages)") && !prompt(0).contains("a1"));
    assert_eq!(prompt(1), person_prompt(&transcript, PersonHistory::Full));
    assert!(prompt(1).contains("ASSISTANT: a1") && !prompt(1).contains("earlier message"));
    // The persona stays in the system prompt either way.
    for i in 0..2 {
        assert!(requests[i]["messages"][0]["content"].as_str().unwrap().contains("Finance owns revenue."));
    }
}
