//! Cloudflare quick-tunnel ("TryCloudflare") manager.
//!
//! Spawns one `cloudflared tunnel --url http://127.0.0.1:<port>` process per
//! forwarded port to expose a VM dev server on a public `https://*.trycloudflare.com`
//! URL — no Cloudflare account or domain required. The assigned URL is parsed
//! from cloudflared's stderr. These URLs are publicly reachable by anyone who
//! has the (random, unguessable) link, so they are intended for previewing
//! landing pages, not for anything sensitive.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

struct Tunnel {
    url: String,
    child: Child,
}

/// Owns the live `cloudflared` processes, keyed by the local port they expose.
pub struct CloudflaredManager {
    tunnels: Mutex<HashMap<u16, Tunnel>>,
}

impl CloudflaredManager {
    /// Create an empty manager (no tunnels running).
    pub fn new() -> Self {
        Self {
            tunnels: Mutex::new(HashMap::new()),
        }
    }

    /// Ensure a quick tunnel exists for `port`, returning its public URL. Reuses
    /// a live tunnel; respawns if the previous process died.
    pub async fn open(&self, port: u16) -> Result<String, String> {
        let mut map = self.tunnels.lock().await;

        if let Some(t) = map.get_mut(&port) {
            match t.child.try_wait() {
                Ok(None) => return Ok(t.url.clone()),
                _ => {
                    map.remove(&port);
                }
            }
        }

        let bin = std::env::var("CLOUDFLARED_BIN").unwrap_or_else(|_| "cloudflared".to_string());
        let mut child = Command::new(&bin)
            .arg("tunnel")
            .arg("--no-autoupdate")
            .arg("--url")
            .arg(format!("http://127.0.0.1:{port}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to start cloudflared ({bin}): {e}"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "cloudflared: no stderr handle".to_string())?;

        // Drain stderr; signal the first trycloudflare URL we see. Keep reading
        // afterward so the pipe never fills and stalls cloudflared.
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tx = Some(tx);
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(url) = extract_trycloudflare_url(&line) {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(url);
                    }
                }
            }
        });

        let url = match tokio::time::timeout(Duration::from_secs(25), rx).await {
            Ok(Ok(url)) => url,
            _ => {
                let _ = child.start_kill();
                return Err("timed out waiting for the cloudflared tunnel URL".to_string());
            }
        };

        map.insert(port, Tunnel { url: url.clone(), child });
        Ok(url)
    }

    /// Tear down the tunnel for `port`, if any.
    pub async fn close(&self, port: u16) {
        if let Some(mut t) = self.tunnels.lock().await.remove(&port) {
            let _ = t.child.start_kill();
        }
    }

    /// List the currently-running tunnels as `(port, url)` pairs.
    pub async fn list(&self) -> Vec<(u16, String)> {
        self.tunnels
            .lock()
            .await
            .iter()
            .map(|(p, t)| (*p, t.url.clone()))
            .collect()
    }
}

impl Default for CloudflaredManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract a `https://<sub>.trycloudflare.com` URL from a cloudflared log line.
fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let idx = line.find("https://")?;
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches('/');
    if url.contains("trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}
