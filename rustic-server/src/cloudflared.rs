//! Cloudflare quick-tunnel ("TryCloudflare") manager.
//!
//! Spawns one `cloudflared tunnel --url http://127.0.0.1:<port>` process per
//! forwarded port to expose a VM dev server on a public `https://*.trycloudflare.com`
//! URL — no Cloudflare account or domain required. The assigned URL is parsed
//! from cloudflared's stderr. These URLs are publicly reachable by anyone who
//! has the (random, unguessable) link, so they are intended for previewing
//! landing pages, not for anything sensitive.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
    /// Loopback ports cloudflared opens for its own metrics endpoint, scraped
    /// from its logs. The port monitor must NOT treat these as user dev servers
    /// (else auto-expose would tunnel cloudflared's own metrics in a loop).
    metrics_ports: Arc<std::sync::Mutex<HashSet<u16>>>,
}

impl CloudflaredManager {
    /// Create an empty manager (no tunnels running).
    pub fn new() -> Self {
        Self {
            tunnels: Mutex::new(HashMap::new()),
            metrics_ports: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }

    /// Ports currently exposed by a live tunnel.
    pub async fn managed_ports(&self) -> Vec<u16> {
        self.tunnels.lock().await.keys().copied().collect()
    }

    /// Loopback ports cloudflared uses for metrics (to exclude from auto-expose).
    pub fn metrics_ports(&self) -> Vec<u16> {
        self.metrics_ports
            .lock()
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
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
        tracing::info!(port, bin = %bin, "cloudflared: launching quick tunnel");
        let mut child = Command::new(&bin)
            .arg("tunnel")
            .arg("--no-autoupdate")
            // Force the HTTP/2 (TCP 443) edge transport. The default QUIC
            // transport needs outbound UDP/7844, which many hosts (Railway
            // among them) block — there cloudflared prints a URL but never
            // registers an edge connection, so the hostname resolves to nothing
            // (DNS_PROBE_FINISHED_NXDOMAIN) and the preview is dead on arrival.
            .arg("--protocol")
            .arg("http2")
            .arg("--url")
            .arg(format!("http://127.0.0.1:{port}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                tracing::error!(port, bin = %bin, error = %e, "cloudflared: spawn failed (is the binary installed?)");
                format!("failed to start cloudflared ({bin}): {e}")
            })?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "cloudflared: no stderr handle".to_string())?;

        // Drain stderr; resolve only once we have BOTH the assigned URL and a
        // registered edge connection. Returning on the URL alone hands back a
        // hostname that isn't live yet (or never will be, if registration
        // fails) — the source of the NXDOMAIN dead links. Keep reading after so
        // the pipe never fills and stalls cloudflared.
        let (tx, rx) = oneshot::channel();
        let metrics_ports = self.metrics_ports.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tx = Some(tx);
            let mut url: Option<String> = None;
            while let Ok(Some(line)) = lines.next_line().await {
                // Surface cloudflared's own output in the server logs — this is
                // the only window into why a tunnel does or doesn't come up.
                tracing::info!(target: "cloudflared", port, "{line}");
                // Record the metrics port so auto-expose never tunnels it.
                if let Some(mp) = extract_metrics_port(&line) {
                    if let Ok(mut s) = metrics_ports.lock() {
                        s.insert(mp);
                    }
                }
                if url.is_none() {
                    if let Some(u) = extract_trycloudflare_url(&line) {
                        tracing::info!(port, url = %u, "cloudflared: tunnel URL assigned");
                        url = Some(u);
                    }
                }
                // cloudflared logs `Registered tunnel connection connIndex=0 …`
                // once an edge connection is actually up and serving. Match THAT
                // line specifically — `connIndex=` alone also appears in earlier
                // pre-registration lines (e.g. "Tunnel connection curve
                // preferences"), which would hand back the URL a beat before the
                // tunnel is really ready and make the first request fail.
                let registered = line.to_lowercase().contains("registered tunnel connection");
                if registered {
                    if let (Some(u), Some(tx)) = (url.clone(), tx.take()) {
                        tracing::info!(port, url = %u, "cloudflared: edge connection registered — tunnel live");
                        let _ = tx.send(u);
                    }
                }
            }
            tracing::warn!(port, "cloudflared: stderr stream ended (process exited)");
        });

        let url = match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(url)) => url,
            _ => {
                tracing::error!(
                    port,
                    "cloudflared: no edge connection registered within 30s — killing it. \
                     Likely the host is blocking cloudflared's outbound edge connection."
                );
                let _ = child.start_kill();
                return Err(
                    "cloudflared did not establish a tunnel (no edge connection registered). \
                     The host may be blocking cloudflared's outbound connection."
                        .to_string(),
                );
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

/// Extract cloudflared's metrics port from a line like
/// `Starting metrics server on 127.0.0.1:20241/metrics`.
fn extract_metrics_port(line: &str) -> Option<u16> {
    let anchor = line.find("metrics server on")?;
    let rest = &line[anchor..];
    // First colon after the anchor separates the (IPv4) host from the port.
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
    after[..end].parse().ok()
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
