use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

async fn read_frame(stream: &mut DuplexStream) -> Option<Value> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).await.ok()?;
    let mut body = vec![0u8; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

async fn write_frame(stream: &mut DuplexStream, value: Value) {
    let bytes = value.to_string().into_bytes();
    stream.write_all(&(bytes.len() as u32).to_le_bytes()).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
}

#[tokio::test]
async fn two_sessions_share_one_host_without_stealing_replies() {
    let home = std::env::temp_dir().join(format!("corex-routing-test-{}", std::process::id()));
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    let hosts = home.join("data/corex/hosts");

    let (host_stdin, mut to_host) = tokio::io::duplex(1 << 20);
    let (mut from_host, host_stdout) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move { corex_browser::run_host(host_stdin, host_stdout, &hosts).await });

    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut next_tab = 6;
        while let Some(msg) = read_frame(&mut from_host).await {
            let session = msg["sessionId"].as_str().unwrap_or("").to_string();
            let result = match msg["method"].as_str().unwrap() {
                "Corex.openTab" => {
                    next_tab += 1;
                    json!({"tabId": next_tab})
                }
                "Runtime.evaluate" => {
                    json!({"result": {"value": format!("{{\"url\":\"https://x.test/\",\"title\":\"Tab {session}\"}}")}})
                }
                "Corex.closeTab" => {
                    let _ = closed_tx.send(msg["params"]["tabId"].as_i64().unwrap());
                    json!({})
                }
                _ => json!({}),
            };
            if msg.get("id").is_some() {
                write_frame(&mut to_host, json!({"id": msg["id"], "result": result})).await;
            }
        }
    });

    let first = corex_browser::Browser::start().await.expect("first session attaches");
    let second = corex_browser::Browser::start().await.expect("second session attaches");
    assert!(first.page_info().await.contains("Tab 7"));
    assert!(second.page_info().await.contains("Tab 8"));
    assert!(first.is_alive() && second.is_alive());

    drop(first);
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), closed_rx.recv()).await.unwrap();
    assert_eq!(closed, Some(7));
    assert!(second.page_info().await.contains("Tab 8"));
    let _ = std::fs::remove_dir_all(home);
}
