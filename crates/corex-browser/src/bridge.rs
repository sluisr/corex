//! Client side of the bridge: finds the socket of a running native host (started by the browser
//! for the Corex extension) and talks Chrome DevTools Protocol through it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::net::UnixStream;

use crate::cdp::Cdp;
use crate::host::{hosts_dir, install_native_host};

const WAIT: Duration = Duration::from_secs(25);

async fn try_hosts() -> Option<Arc<Cdp>> {
    let mut sockets: Vec<_> = std::fs::read_dir(hosts_dir())
        .ok()?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sock"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    sockets.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in sockets {
        match UnixStream::connect(&path).await {
            Ok(stream) => {
                if let Ok((socket, _)) = tokio_tungstenite::client_async("ws://corex.local/", stream).await {
                    return Some(Cdp::from_socket(socket));
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    None
}

pub async fn connect() -> Result<Arc<Cdp>> {
    let _ = install_native_host();
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(cdp) = try_hosts().await {
            return Ok(cdp);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "the Corex Browser Bridge extension is not running. Open the browser where it is installed \
                 and make sure the extension is enabled and up to date. To install it: https://chromewebstore.google.com/detail/pgbencaoenigjeekljfindfeaolenaip"
            ));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
