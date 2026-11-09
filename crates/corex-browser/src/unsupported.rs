use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::cdp::Cdp;

pub async fn connect() -> Result<Arc<Cdp>> {
    Err(anyhow!("the browser bridge is not supported on this platform yet"))
}

pub fn install_native_host() -> Result<()> {
    Ok(())
}

pub async fn run_native_host() -> Result<()> {
    Err(anyhow!("the browser bridge is not supported on this platform yet"))
}

pub async fn run_host<R, W>(_stdin: R, _stdout: W, _dir: &std::path::Path) -> Result<()> {
    Err(anyhow!("the browser bridge is not supported on this platform yet"))
}
