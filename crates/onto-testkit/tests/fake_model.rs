//! The model loop against a scripted OpenAI-compatible server, so the protocol
//! code is tested on every PR without downloading a model.

#![cfg(feature = "llm")]

use std::sync::{Arc, Mutex};

use onto_testkit::agent::{ModelConfig, Reply, ScriptedHuman, run_episode};
use onto_testkit::{Event, Fixture, Session, invariants};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } }] })
}

/// Serve canned assistant messages in order; record every request body.
async fn fake_model(responses: Vec<Value>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for message in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let body = loop {
                let mut chunk = [0u8; 8192];
                let n = socket.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let length: usize = text[..split]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                        .unwrap_or(0);
                    if buf.len() >= split + 4 + length {
                        break serde_json::from_slice::<Value>(&buf[split + 4..split + 4 + length]).unwrap();
                    }
                }
            };
            log.lock().unwrap().push(body);
            let payload = json!({ "choices": [{ "message": message }] }).to_string();
            let response =
                format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}", payload.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, seen)
}

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
