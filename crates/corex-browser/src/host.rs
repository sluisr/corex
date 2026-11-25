//! Native Messaging host. The browser launches it for the Corex extension; it relays frames between
//! the extension (stdio) and Corex (a private unix socket), so no ports and no pairing are involved.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub const HOST_NAME: &str = "com.corex.bridge";
const EXTENSION_ID: &str = crate::diagnostics::EXTENSION_ID;
const MAX_FRAME: usize = 64 * 1024 * 1024;
const MAX_HOST_LOG_BYTES: u64 = 64 * 1024;

pub(crate) fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "corex")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("corex"))
}

pub(crate) fn hosts_dir() -> PathBuf {
    data_dir().join("hosts")
}

pub(crate) fn host_log_path() -> PathBuf {
    data_dir().join("browser-host.log")
}

pub(crate) fn append_host_log(message: &str) {
    use std::io::Write;
    let path = host_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_HOST_LOG_BYTES) {
        let _ = std::fs::write(&path, "");
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[ts={secs} pid={}] {message}", std::process::id());
    }
}

#[derive(Default)]
struct Router {
    clients: HashMap<u64, mpsc::UnboundedSender<String>>,
    pending: HashMap<u64, (u64, Value, String)>,
    owners: HashMap<i64, u64>,
    next_id: u64,
}

impl Router {
    fn route_to_browser(&mut self, client: u64, text: &str) -> Option<String> {
        let mut message: Value = serde_json::from_str(text).ok()?;
        if let Some(id) = message.get("id").cloned() {
            self.next_id += 1;
            let method = message["method"].as_str().unwrap_or("").to_string();
            self.pending.insert(self.next_id, (client, id, method));
            message["id"] = json!(self.next_id);
        }
        Some(message.to_string())
    }

    fn route_to_client(&mut self, text: &str) {
        let Ok(mut message) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let target = match message.get("id").and_then(Value::as_u64) {
            Some(id) => {
                let Some((client, original, method)) = self.pending.remove(&id) else {
                    return;
                };
                if method == "Corex.openTab" {
                    if let Some(tab) = message["result"]["tabId"].as_i64() {
                        self.owners.insert(tab, client);
                    }
                }
                message["id"] = original;
                Some(client)
            }
            None => {
                let params = &message["params"];
                match message["method"].as_str() {
                    Some("Corex.tabOpened") => {
                        let owner = params["openerTabId"]
                            .as_i64()
                            .and_then(|opener| self.owners.get(&opener).copied());
                        if let (Some(owner), Some(tab)) = (owner, params["tabId"].as_i64()) {
                            self.owners.insert(tab, owner);
                        }
                        owner
                    }
                    Some("Corex.tabClosed") => params["tabId"]
                        .as_i64()
                        .and_then(|tab| self.owners.remove(&tab)),
                    _ => message["sessionId"]
                        .as_str()
                        .and_then(|session| session.split(':').next()?.parse::<i64>().ok())
                        .and_then(|tab| self.owners.get(&tab).copied()),
                }
            }
        };
        if let Some(sender) = target.and_then(|client| self.clients.get(&client)) {
            let _ = sender.send(message.to_string());
        }
    }

    fn drop_client(&mut self, client: u64) -> Vec<i64> {
        self.clients.remove(&client);
        self.pending.retain(|_, (owner, _, _)| *owner != client);
        let tabs: Vec<i64> = self
            .owners
            .iter()
            .filter(|(_, owner)| **owner == client)
            .map(|(tab, _)| *tab)
            .collect();
        self.owners.retain(|_, owner| *owner != client);
        tabs
    }
}

type Shared = Arc<Mutex<Router>>;

pub async fn run_native_host() -> Result<()> {
    append_host_log("native host started");
    let res = run_host(tokio::io::stdin(), tokio::io::stdout(), &hosts_dir()).await;
    match &res {
        Ok(()) => append_host_log("native host exited cleanly (browser closed stdio)"),
        Err(e) => append_host_log(&format!("native host error: {e}")),
    }
    res
}

pub async fn run_host<R, W>(mut stdin: R, stdout: W, dir: &Path) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    std::fs::create_dir_all(dir)?;
    set_mode(dir, 0o700);
    let socket = dir.join(format!("{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    set_mode(&socket, 0o600);

    let (to_browser, mut browser_queue) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut stdout = stdout;
        while let Some(text) = browser_queue.recv().await {
            let bytes = text.as_bytes();
            let framed = [&(bytes.len() as u32).to_le_bytes()[..], bytes].concat();
            if stdout.write_all(&framed).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let router: Shared = Arc::default();
    let accepting = tokio::spawn({
        let router = router.clone();
        async move {
            let mut next_client = 0u64;
            while let Ok((stream, _)) = listener.accept().await {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    continue;
                };
                next_client += 1;
                let client = next_client;
                let (mut sink, mut incoming) = ws.split();
                let (to_client, mut client_queue) = mpsc::unbounded_channel::<String>();
                if let Ok(mut router) = router.lock() {
                    router.clients.insert(client, to_client);
                }
                tokio::spawn(async move {
                    while let Some(text) = client_queue.recv().await {
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                });
                let to_browser = to_browser.clone();
                let router = router.clone();
                tokio::spawn(async move {
                    while let Some(Ok(message)) = incoming.next().await {
                        match message {
                            Message::Text(text) => {
                                let routed = router
                                    .lock()
                                    .ok()
                                    .and_then(|mut r| r.route_to_browser(client, text.as_str()));
                                if let Some(routed) = routed {
                                    let _ = to_browser.send(routed);
                                }
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                    let orphans = router
                        .lock()
                        .map(|mut r| r.drop_client(client))
                        .unwrap_or_default();
                    for tab in orphans {
                        let _ = to_browser.send(
                            json!({ "method": "Corex.closeTab", "params": { "tabId": tab } })
                                .to_string(),
                        );
                    }
                });
            }
        }
    });

    let outcome = relay_browser_frames(&mut stdin, &router).await;
    accepting.abort();
    let _ = std::fs::remove_file(&socket);
    outcome
}

async fn relay_browser_frames<R: AsyncRead + Unpin>(stdin: &mut R, router: &Shared) -> Result<()> {
    loop {
        let mut length = [0u8; 4];
        if stdin.read_exact(&mut length).await.is_err() {
            return Ok(());
        }
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_FRAME {
            return Err(anyhow!("the browser sent a {length}-byte message"));
        }
        let mut body = vec![0u8; length];
        stdin.read_exact(&mut body).await?;
        if let Ok(mut router) = router.lock() {
            router.route_to_client(&String::from_utf8_lossy(&body));
        }
    }
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

pub(crate) fn browser_config_dirs_named() -> Vec<(String, PathBuf)> {
    let Some(base) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let mut candidates: Vec<(&str, PathBuf)> = if cfg!(target_os = "macos") {
        let root = base.home_dir().join("Library/Application Support");
        vec![
            ("Google Chrome", root.join("Google/Chrome")),
            ("Chromium", root.join("Chromium")),
            ("Brave", root.join("BraveSoftware/Brave-Browser")),
            ("Microsoft Edge", root.join("Microsoft Edge")),
            ("Vivaldi", root.join("Vivaldi")),
        ]
    } else {
        let cfg = base.home_dir().join(".config");
        let flatpak = base.home_dir().join(".var/app");
        vec![
            ("Google Chrome", cfg.join("google-chrome")),
            ("Chromium", cfg.join("chromium")),
            ("Brave", cfg.join("BraveSoftware/Brave-Browser")),
            ("Microsoft Edge", cfg.join("microsoft-edge")),
            ("Vivaldi", cfg.join("vivaldi")),
            ("Google Chrome (Flatpak)", flatpak.join("com.google.Chrome/config/google-chrome")),
            ("Chromium (Flatpak)", flatpak.join("org.chromium.Chromium/config/chromium")),
            ("Brave (Flatpak)", flatpak.join("com.brave.Browser/config/BraveSoftware/Brave-Browser")),
            ("Microsoft Edge (Flatpak)", flatpak.join("com.microsoft.Edge/config/microsoft-edge")),
        ]
    };
    candidates
        .drain(..)
        .filter(|(_, dir)| dir.is_dir())
        .map(|(name, dir)| (name.to_string(), dir))
        .collect()
}

pub(crate) fn launcher_path() -> PathBuf {
    data_dir().join("corex-browser-host")
}

fn write_if_changed(path: &Path, content: &str, mode: u32) -> Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    std::fs::write(path, content)?;
    set_mode(path, mode);
    Ok(())
}

pub fn install_native_host() -> Result<()> {
    let exe = std::env::current_exe()?;
    let root = data_dir();
    std::fs::create_dir_all(&root)?;
    let launcher = launcher_path();
    write_if_changed(
        &launcher,
        &format!(
            "#!/bin/sh\nexec '{}' browser-host \"$@\"\n",
            exe.to_string_lossy().replace('\'', "'\\''")
        ),
        0o755,
    )?;
    let manifest = serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "Corex Browser Bridge",
        "path": launcher,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    }))?;
    for (_, config) in browser_config_dirs_named() {
        let hosts = config.join("NativeMessagingHosts");
        std::fs::create_dir_all(&hosts)?;
        write_if_changed(&hosts.join(format!("{HOST_NAME}.json")), &manifest, 0o644)?;
    }
    Ok(())
}
