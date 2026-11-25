use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::cdp::Cdp;

pub async fn connect() -> Result<Arc<Cdp>> {
    Err(anyhow!(
        "the browser bridge is not supported on this platform yet"
    ))
}

pub fn install_native_host() -> Result<()> {
    Ok(())
}

pub async fn run_native_host() -> Result<()> {
    Err(anyhow!(
        "the browser bridge is not supported on this platform yet"
    ))
}

pub async fn diagnose() -> crate::diagnostics::BrowserDiagnostics {
    crate::diagnostics::BrowserDiagnostics {
        browsers_detected: Vec::new(),
        extensions: Vec::new(),
        native_host_launcher: std::path::PathBuf::new(),
        launcher_ok: false,
        launcher_issue: Some("the browser bridge is not supported on this platform yet".into()),
        manifests_installed: Vec::new(),
        install_error: Some("the browser bridge is not supported on this platform yet".into()),
        live_socket: None,
        last_host_log: Vec::new(),
    }
}

pub async fn run_host<R, W>(_stdin: R, _stdout: W, _dir: &std::path::Path) -> Result<()> {
    Err(anyhow!(
        "the browser bridge is not supported on this platform yet"
    ))
}
