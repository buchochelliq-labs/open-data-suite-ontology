//! The model loop against a scripted OpenAI-compatible server, so the protocol
//! code is tested on every PR without downloading a model.

#![cfg(feature = "llm")]

use onto_testkit::agent::{ModelConfig, Reply, ScriptedHuman, run_episode};
use onto_testkit::fake_model::{error, say, serve as fake_model, tool_call};
use onto_testkit::{Event, Fixture, Session, invariants};
use serde_json::json;

#[tokio::test]
async fn model_loop_drives_tools_and_respects_the_human() {
    let (url, seen) = fake_model(vec![
        tool_call("1", "ontology_search", json!({ "query": "customer" })),
        tool_call("2", "ontology_entity", json!({ "id": "concept:customer" })),
        json!({ "role": "assistant", "content": "The repository defines a customer as an account holder; the central pack differs." }),
        tool_call("3", "ontology_propose", json!({ "subject": "concept:customer", "predicate": "grain", "object": { "literal": "one row per person" }, "rationale": "User decision in session." })),
        // Small models sometimes emit malformed arguments.
        json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": "4", "type": "function", "function": { "name": "ontology_entity", "arguments": "{\"id\": " } }] }),
        json!({ "role": "assistant", "content": "I prepared a proposal; it is pending your review." }),
        json!({ "role": "assistant", "content": "Thanks, it is now accepted." }),
    ])
    .await;

    let fixture = Fixture::new("shop").unwrap();
    let before = fixture.repository_files().unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let config = ModelConfig {
        base_url: url,
        model: "fake".into(),
        api_key: None,
        max_turns: 10,
        max_tool_result_chars: 300,
        timeout: std::time::Duration::from_secs(10),
    };
    let mut human =
        ScriptedHuman::new([Reply::Say("Propose grain one row per person.".into()), Reply::AcceptLastProposal { by: "reviewer".into() }, Reply::End]);

    let stats = run_episode(&mut session, &config, "system", "What is a customer?", &mut human).await.unwrap();
    let transcript = session.close().await.unwrap();

    assert_eq!(stats.tool_calls, 4);
    assert_eq!(stats.failed_tool_calls, 1, "malformed arguments are reported back, not fatal");
    assert!(transcript.events.iter().any(|e| matches!(e, Event::HumanAction { action, .. } if action == "accept")));
    invariants::check(&transcript, &before, &fixture.repository_files().unwrap()).assert_clean();

    let requests = seen.lock().unwrap();
    let first = &requests[0];
    let tool_names: Vec<&str> = first["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(tool_names.contains(&"ontology_propose"));
    assert!(first["messages"][0]["content"].as_str().unwrap().contains("untrusted evidence"), "server instructions reach the model");
    let tool_message = requests[1]["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap();
    assert!(tool_message["content"].as_str().unwrap().contains("[truncated"), "tool results are budgeted");
}

#[tokio::test]
async fn rate_limits_are_retried_and_persistent_errors_reported() {
    let (url, seen) = fake_model(vec![error(429, Some("0")), error(503, Some("0")), say("Hello.")]).await;
    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let config = ModelConfig {
        base_url: url,
        model: "fake".into(),
        api_key: None,
        max_turns: 2,
        max_tool_result_chars: 300,
        timeout: std::time::Duration::from_secs(10),
    };
    run_episode(&mut session, &config, "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 3, "two retries, then the answer");
    assert_eq!(session.close().await.unwrap().agent_texts().collect::<Vec<_>>(), vec!["Hello."]);

    let (url, _) = fake_model(vec![error(400, None)]).await;
    let mut session = Session::start(&fixture).await.unwrap();
    let config = ModelConfig { base_url: url, ..config };
    let err = run_episode(&mut session, &config, "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap_err();
    assert!(err.to_string().contains("400"), "{err}");
}
