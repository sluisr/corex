//! Client side of the bridge: finds the socket of a running native host (started by the browser
//! for the Corex extension) and talks Chrome DevTools Protocol through it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::net::UnixStream;

use crate::cdp::Cdp;
use crate::diagnostics::{
    check_launcher, find_extensions_in_browser_dir, read_tail_lines, BrowserDiagnostics,
};
use crate::host::{
    browser_config_dirs_named, host_log_path, hosts_dir, install_native_host, launcher_path,
    HOST_NAME,
};

const WAIT_INSTALLED: Duration = Duration::from_secs(12);
const WAIT_UNINSTALLED: Duration = Duration::from_millis(1500);

async fn try_hosts_with_path() -> Option<(Arc<Cdp>, PathBuf)> {
    let dir = hosts_dir();
    let sockets: Vec<PathBuf> = tokio::task::spawn_blocking(move || {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut list: Vec<_> = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sock"))
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        list.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        list.into_iter().map(|(_, p)| p).collect()
    })
    .await
    .unwrap_or_default();

    for path in sockets {
        match UnixStream::connect(&path).await {
            Ok(stream) => {
                if let Ok((socket, _)) =
                    tokio_tungstenite::client_async("ws://corex.local/", stream).await
                {
                    return Some((Cdp::from_socket(socket), path));
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    None
}

async fn try_hosts() -> Option<Arc<Cdp>> {
    try_hosts_with_path().await.map(|(cdp, _)| cdp)
}

/// Inspects browsers, installed Corex extension profiles, native host launcher/manifests,
/// active socket connectivity, and recent native host logs (read-only; use `/browser repair`
/// or `install_native_host()` to rewrite manifests/launcher).
pub async fn diagnose() -> BrowserDiagnostics {
    let live_socket = try_hosts_with_path().await.map(|(_, path)| path);
    collect_disk_diagnostics(None, live_socket).await
}

async fn collect_disk_diagnostics(
    install_error: Option<String>,
    live_socket: Option<PathBuf>,
) -> BrowserDiagnostics {
    tokio::task::spawn_blocking(move || {
        let browsers_detected = browser_config_dirs_named();
        let mut extensions = Vec::new();
        let mut manifests_installed = Vec::new();
        for (name, dir) in &browsers_detected {
            extensions.extend(find_extensions_in_browser_dir(name, dir));
            let manifest = dir.join("NativeMessagingHosts").join(format!("{HOST_NAME}.json"));
            if manifest.is_file() {
                manifests_installed.push(manifest);
            }
        }
        let native_host_launcher = launcher_path();
        let (launcher_ok, launcher_issue) = check_launcher(&native_host_launcher);
        let last_host_log = read_tail_lines(&host_log_path(), 6);
        BrowserDiagnostics {
            browsers_detected,
            extensions,
            native_host_launcher,
            launcher_ok,
            launcher_issue,
            manifests_installed,
            install_error,
            live_socket,
            last_host_log,
        }
    })
    .await
    .unwrap_or_else(|_| BrowserDiagnostics {
        browsers_detected: Vec::new(),
        extensions: Vec::new(),
        native_host_launcher: launcher_path(),
        launcher_ok: false,
        launcher_issue: Some("diagnostic task panicked".into()),
        manifests_installed: Vec::new(),
        install_error: None,
        live_socket: None,
        last_host_log: Vec::new(),
    })
}

pub async fn connect() -> Result<Arc<Cdp>> {
    let install_error = tokio::task::spawn_blocking(install_native_host)
        .await
        .ok()
        .and_then(|res| res.err())
        .map(|e| e.to_string());

    if let Some(cdp) = try_hosts().await {
        return Ok(cdp);
    }

    let initial_diag = collect_disk_diagnostics(install_error.clone(), None).await;
    // If no browser has the extension installed, don't block the user for 12–25s waiting
    // on a socket that cannot appear; allow a brief 1.5s window in case of an unpacked dev build.
    let wait_budget = if initial_diag.extensions.is_empty() && !initial_diag.browsers_detected.is_empty() {
        WAIT_UNINSTALLED
    } else {
        WAIT_INSTALLED
    };

    let deadline = Instant::now() + wait_budget;
    loop {
        if let Some(cdp) = try_hosts().await {
            return Ok(cdp);
        }
        if Instant::now() >= deadline {
            let final_diag = collect_disk_diagnostics(install_error, None).await;
            return Err(anyhow!("{}", final_diag.diagnosis_error()));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
