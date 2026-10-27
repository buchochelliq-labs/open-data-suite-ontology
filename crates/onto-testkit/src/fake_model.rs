//! A scripted OpenAI-compatible endpoint, so model-driven code is tested
//! without downloading or calling a model.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// An assistant message that calls one tool.
pub fn tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } }] })
}

/// An assistant message with text only.
pub fn say(text: &str) -> Value {
    json!({ "role": "assistant", "content": text })
}

/// A scripted HTTP error response, e.g. a rate limit.
pub fn error(status: u16, retry_after: Option<&str>) -> Value {
    json!({ "__status": status, "__retry_after": retry_after })
}

/// Serve `responses` in order, one per request; returns the base URL and a log of request bodies.
pub async fn serve(responses: Vec<Value>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a local port");
    let url = format!("http://{}/v1", listener.local_addr().expect("local address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for message in responses {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let mut buf = Vec::new();
            let body = loop {
                let mut chunk = [0u8; 8192];
                let Ok(n) = socket.read(&mut chunk).await else { return };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let length: usize = text[..split]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse().ok()))
                        .unwrap_or(0);
                    if buf.len() >= split + 4 + length {
                        break serde_json::from_slice::<Value>(&buf[split + 4..split + 4 + length]).unwrap_or(Value::Null);
                    }
                }
            };
            log.lock().expect("request log").push(body);
            let response = match message.get("__status").and_then(Value::as_u64) {
                // A scripted error, e.g. `{"__status": 429, "__retry_after": "0"}`.
                Some(status) => {
                    let retry = message.get("__retry_after").and_then(Value::as_str).map(|r| format!("retry-after: {r}\r\n")).unwrap_or_default();
                    let payload = json!({ "error": { "message": "scripted error" } }).to_string();
                    format!(
                        "HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\n{retry}content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len()
                    )
                }
                None => {
                    let payload = json!({ "choices": [{ "message": message }] }).to_string();
                    format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}", payload.len())
                }
            };
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (url, seen)
}
