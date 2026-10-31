use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use corex_core::client::{LlmClient, StreamEvent};
use corex_core::config::Config;
use corex_core::types::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// A tiny HTTP server: the first `failures` requests get `status`, the rest a short SSE answer.
async fn flaky_server(failures: usize, status: &'static str) -> (String, Arc<AtomicUsize>) {
    flaky_server_with_body(failures, status, r#"{"error":{"message":"slow down"}}"#).await
}

async fn flaky_server_with_body(failures: usize, status: &'static str, error_body: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let read = sock.read(&mut buf).await.unwrap_or(0);
                // The client also asks for model metadata; only chat requests are attempts.
                if !String::from_utf8_lossy(&buf[..read]).starts_with("POST") {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                    return;
                }
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let reply = if n < failures {
                    let body = error_body;
                    format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                } else {
                    let events = [
                        r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"hola"},"finish_reason":null}]}"#,
                        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                        "[DONE]",
                    ];
                    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
                    format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                };
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (url, hits)
}

fn client(url: &str) -> LlmClient {
    let cfg = Config {
        base_url: url.to_string(),
        api_key: "test-key".to_string(),
        model: "test-model".to_string(),
        active_provider: None,
        local_llm_enabled: false,
        ..Config::default()
    };
    LlmClient::new(cfg)
}

async fn collect(client: &LlmClient, cancel: CancellationToken) -> (String, Vec<String>, Vec<String>) {
    let mut rx = client.stream_chat(vec![Message::user("hi")], None, cancel).await.unwrap();
    let (mut text, mut notices, mut errors) = (String::new(), Vec::new(), Vec::new());
    while let Some(ev) = rx.recv().await {
        match ev {
            StreamEvent::ContentDelta(t) => text.push_str(&t),
            StreamEvent::Notice(n) => notices.push(n),
            StreamEvent::Error(e) => errors.push(e),
            _ => {}
        }
    }
    (text, notices, errors)
}

#[tokio::test]
async fn a_rate_limit_before_any_content_is_retried() {
    let (url, hits) = flaky_server(1, "429 Too Many Requests").await;
    let (text, notices, errors) = collect(&client(&url), CancellationToken::new()).await;
    assert_eq!(text, "hola", "errors: {errors:?}");
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(notices.iter().any(|n| n.contains("429") && n.contains("Retried")), "{notices:?}");
}

#[tokio::test]
async fn a_spent_daily_quota_is_reported_instead_of_retried() {
    let google = r#"[{"error":{"code":429,"details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"44632s"}]}}]"#;
    let (url, hits) = flaky_server_with_body(10, "429 Too Many Requests", google).await;
    let started = std::time::Instant::now();
    let (_, _, errors) = collect(&client(&url), CancellationToken::new()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1, "{errors:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert!(errors.iter().any(|e| e.contains("Quota exhausted") && e.contains("12h 23m")), "{errors:?}");
}

#[tokio::test]
async fn a_bad_key_is_not_retried() {
    let (url, hits) = flaky_server(10, "401 Unauthorized").await;
    let (text, _, errors) = collect(&client(&url), CancellationToken::new()).await;
    assert!(text.is_empty());
    assert!(errors.iter().any(|e| e.contains("401")), "{errors:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "a 401 must fail at once");
}

#[tokio::test]
async fn cancelling_stops_the_wait_between_retries() {
    let (url, hits) = flaky_server(10, "503 Service Unavailable").await;
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        stopper.cancel();
    });
    let started = std::time::Instant::now();
    let (_, _, errors) = collect(&client(&url), cancel).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "Esc must not wait out the backoff");
    assert!(!errors.is_empty());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn transient_errors_are_told_apart() {
    use corex_core::client::is_transient_error;
    for m in [
        "Rate limit or quota exceeded (429 Too Many Requests) from Google Gemini.",
        "API error (503 Service Unavailable): try later",
        "API error (529 <unknown status code>): {\"type\":\"overloaded_error\"}",
        "Stream error: error sending request for url",
    ] {
        assert!(is_transient_error(m), "{m}");
    }
    for m in [
        "Quota exhausted (429 Too Many Requests) from Google Gemini: it asks to wait 12h 23m before the next request.",
        "Authentication error (401 Unauthorized): Invalid or missing API key",
        "API error (400 Bad Request): invalid tool schema",
        "This model's maximum context length is 128000 tokens",
    ] {
        assert!(!is_transient_error(m), "{m}");
    }
}
