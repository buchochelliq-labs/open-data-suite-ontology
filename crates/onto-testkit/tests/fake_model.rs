//! The model loop against a scripted OpenAI-compatible server, so the protocol
//! code is tested on every PR without downloading a model.

#![cfg(feature = "llm")]

use onto_testkit::agent::{ModelConfig, Reply, ScriptedHuman, run_episode};
use onto_testkit::agent::{RequestBudget, is_budget_exhausted};
use onto_testkit::fake_model::{error, error_with_body, say, serve as fake_model, tool_call};
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
        requests_per_minute: None,
        budget: None,
    };
    let mut human = ScriptedHuman::new([
        Reply::Say("Propose grain one row per person.".into()),
        Reply::AcceptLastProposal { by: "reviewer".into(), then: None },
        Reply::End,
    ]);

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
    assert!(first["tools"].as_array().unwrap().iter().all(|t| t["function"]["parameters"].get("$schema").is_none()), "schemas are portable");
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
        requests_per_minute: None,
        budget: None,
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

#[tokio::test]
async fn requests_are_paced_under_a_per_minute_limit() {
    let (url, seen) = fake_model(vec![tool_call("1", "ontology_status", json!({})), tool_call("2", "ontology_status", json!({})), say("Done.")]).await;
    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    // 300 a minute is one request every 200 ms.
    let config = ModelConfig {
        base_url: url,
        model: "fake".into(),
        api_key: None,
        max_turns: 5,
        max_tool_result_chars: 300,
        timeout: std::time::Duration::from_secs(10),
        requests_per_minute: Some(300),
        budget: None,
    };
    let started = std::time::Instant::now();
    run_episode(&mut session, &config, "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 3);
    assert!(started.elapsed() >= std::time::Duration::from_millis(400), "three requests need two 200 ms gaps, took {:?}", started.elapsed());
    session.close().await.unwrap();
}

fn quick(url: String) -> ModelConfig {
    ModelConfig {
        base_url: url,
        model: "fake".into(),
        api_key: None,
        max_turns: 5,
        max_tool_result_chars: 300,
        timeout: std::time::Duration::from_secs(10),
        requests_per_minute: None,
        budget: None,
    }
}

/// Google's quota error, as its OpenAI-compatible endpoint returns it.
fn google_quota(quota_id: &str, retry: &str) -> serde_json::Value {
    json!([{ "error": { "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "You exceeded your current quota.",
        "details": [
            { "@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [{ "quotaId": quota_id }] },
            { "@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": retry }
        ] } }])
}

#[tokio::test]
async fn a_daily_quota_stops_at_once_and_a_minute_quota_waits() {
    let fixture = Fixture::new("shop").unwrap();

    let (url, seen) = fake_model(vec![error_with_body(429, google_quota("GenerateRequestsPerDayPerProjectPerModel-FreeTier", "40s")), say("unused")]).await;
    let mut session = Session::start(&fixture).await.unwrap();
    let err = run_episode(&mut session, &quick(url), "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap_err();
    assert!(is_budget_exhausted(&err), "{err:#}");
    assert_eq!(seen.lock().unwrap().len(), 1, "no retries against a daily quota");

    let (url, seen) = fake_model(vec![error_with_body(429, google_quota("GenerateRequestsPerMinutePerProjectPerModel-FreeTier", "0s")), say("Hello.")]).await;
    let mut session = Session::start(&fixture).await.unwrap();
    run_episode(&mut session, &quick(url), "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2, "a per-minute limit is waited out");
}

#[tokio::test]
async fn the_run_budget_caps_requests() {
    let (url, seen) = fake_model(vec![tool_call("1", "ontology_status", json!({})), tool_call("2", "ontology_status", json!({})), say("unused")]).await;
    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let budget = RequestBudget::new(2);
    let config = ModelConfig { budget: Some(budget.clone()), ..quick(url) };
    let err = run_episode(&mut session, &config, "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap_err();
    assert!(is_budget_exhausted(&err), "{err:#}");
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert_eq!(budget.used(), 2);
}

#[tokio::test]
async fn a_provider_that_does_not_answer_is_retried_after_the_timeout() {
    use tokio::net::TcpListener;
    // First connection: accept and never answer. Second: answer normally.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (answer_url, _) = fake_model(vec![say("Hello.")]).await;
    let answer_addr = answer_url.trim_start_matches("http://").trim_end_matches("/v1").to_string();
    tokio::spawn(async move {
        let (_hold, _) = listener.accept().await.unwrap();
        // Forward the next connection to the answering server.
        let (mut inbound, _) = listener.accept().await.unwrap();
        let mut outbound = tokio::net::TcpStream::connect(answer_addr).await.unwrap();
        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
        drop(_hold);
    });
    let fixture = Fixture::new("shop").unwrap();
    let mut session = Session::start(&fixture).await.unwrap();
    let config = ModelConfig { timeout: std::time::Duration::from_millis(300), ..quick(silent_url) };
    run_episode(&mut session, &config, "system", "Hi", &mut ScriptedHuman::new([Reply::End])).await.unwrap();
    assert_eq!(session.close().await.unwrap().agent_texts().collect::<Vec<_>>(), vec!["Hello."]);
}
