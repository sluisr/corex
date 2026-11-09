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
async fn drives_a_tab_through_the_native_host() {
    let home = std::env::temp_dir().join(format!("corex-bridge-test-{}", std::process::id()));
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    let hosts = home.join("data/corex/hosts");

    let (host_stdin, mut to_host) = tokio::io::duplex(1 << 20);
    let (mut from_host, host_stdout) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move { corex_browser::run_host(host_stdin, host_stdout, &hosts).await });

    let fake_extension = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(msg) = read_frame(&mut from_host).await {
            let method = msg["method"].as_str().unwrap().to_string();
            seen.push((method.clone(), msg["sessionId"].clone()));
            let result = match method.as_str() {
                "Corex.openTab" => json!({"tabId": 7}),
                "Runtime.evaluate" => json!({"result": {"value": "{\"url\":\"https://x.test/\",\"title\":\"Fake\"}"}}),
                "Corex.closeTab" => break,
                _ => json!({}),
            };
            write_frame(&mut to_host, json!({"id": msg["id"], "result": result})).await;
        }
        seen
    });

    let browser = corex_browser::Browser::start().await.expect("browser attaches through the host");
    assert!(browser.is_alive());
    assert!(browser.page_info().await.contains("Fake"));
    drop(browser);

    let seen = fake_extension.await.unwrap();
    assert_eq!(seen[0].0, "Corex.openTab");
    assert!(seen.iter().any(|(m, s)| m == "Page.enable" && s == "7"));
    assert!(seen.iter().any(|(m, _)| m == "Corex.closeTab"));
    let _ = std::fs::remove_dir_all(home);
}
