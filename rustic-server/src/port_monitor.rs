//! Watches which TCP ports are listening inside the VM and ties Cloudflare
//! quick-tunnel lifecycle to them:
//!
//! * **Auto-expose** — when a new dev server starts listening (any port that
//!   wasn't up at boot and isn't one of ours), open a public tunnel for it and
//!   announce the URL on the hub (the frontend Tunnels panel + a toast).
//! * **Reap-on-death** — when the server behind a live tunnel's port stops
//!   listening for a grace window, close that tunnel (so a stopped dev server
//!   doesn't leave a public URL serving 502s forever).
//!
//! Linux-only: it reads `/proc/net/tcp{,6}`. On other platforms it no-ops (the
//! whole embedded-browser/tunnel feature only ships on the Linux server image).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::context::ServerContext;

/// How often to scan listening ports.
const POLL: Duration = Duration::from_secs(3);
/// How long a tunnel's upstream port may be gone before the tunnel is reaped.
/// Generous so a dev-server recompile/HMR restart (which briefly drops the
/// port) doesn't tear the tunnel down.
const REAP_GRACE: Duration = Duration::from_secs(45);
/// cloudflared's metrics endpoints land here; never auto-expose this range even
/// before a metrics port has been scraped from the logs.
const METRICS_RANGE: std::ops::Range<u16> = 20240..20260;

/// Spawn the monitor as a background task. `server_port` (the rustic-server bind
/// port) and `cdp_port` (Chromium's debug port) are excluded from auto-expose.
pub fn spawn(ctx: ServerContext, server_port: u16, cdp_port: u16) {
    tokio::spawn(async move { run(ctx, server_port, cdp_port).await });
}

async fn run(ctx: ServerContext, server_port: u16, cdp_port: u16) {
    // Ports already listening at boot are infrastructure, not user dev servers.
    let baseline = listening_ports();
    if baseline.is_empty() && !proc_net_available() {
        tracing::info!("port monitor: /proc/net/tcp unavailable (non-Linux?) — disabled");
        return;
    }
    tracing::info!(?baseline, "port monitor: started");

    // Port -> first time we noticed its upstream had vanished (reap grace clock).
    let mut missing_since: HashMap<u16, Instant> = HashMap::new();
    // Ports we've already acted on this lifetime, so a still-listening server
    // isn't re-exposed every poll.
    let mut handled: HashSet<u16> = HashSet::new();

    loop {
        tokio::time::sleep(POLL).await;
        let current = listening_ports();

        let auto_expose = ctx
            .tunnel
            .read()
            .ok()
            .map(|t| t.auto_expose)
            .unwrap_or(false);

        let metrics: HashSet<u16> = ctx.cloudflared.metrics_ports().into_iter().collect();
        let managed: HashSet<u16> = ctx.cloudflared.managed_ports().await.into_iter().collect();

        let is_ours = |p: u16| -> bool {
            p == server_port
                || p == cdp_port
                || baseline.contains(&p)
                || metrics.contains(&p)
                || METRICS_RANGE.contains(&p)
        };

        // 1) New dev servers → auto-expose (when enabled).
        if auto_expose {
            for &p in &current {
                if is_ours(p) || managed.contains(&p) || handled.contains(&p) {
                    continue;
                }
                handled.insert(p);
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    match ctx.cloudflared.open(p).await {
                        Ok(url) => {
                            tracing::info!(port = p, %url, "port monitor: auto-exposed dev server");
                            ctx.hub
                                .publish("tunnel-opened", json!({ "port": p, "url": url }));
                        }
                        Err(e) => {
                            tracing::warn!(port = p, error = %e, "port monitor: auto-expose failed");
                        }
                    }
                });
            }
        }

        // 2) Live tunnels whose upstream port is gone → reap after the grace.
        for p in &managed {
            if current.contains(p) {
                missing_since.remove(p);
            } else {
                let first = *missing_since.entry(*p).or_insert_with(Instant::now);
                if first.elapsed() >= REAP_GRACE {
                    ctx.cloudflared.close(*p).await;
                    missing_since.remove(p);
                    handled.remove(p);
                    tracing::info!(port = p, "port monitor: closed tunnel — upstream server gone");
                    ctx.hub.publish("tunnel-closed", json!({ "port": p }));
                }
            }
        }

        // Forget ports that are neither listening nor pending reap, so a later
        // restart of the same port is treated as a fresh dev server.
        handled.retain(|p| current.contains(p) || missing_since.contains_key(p));
    }
}

/// True if `/proc/net/tcp` can be read at all (used to distinguish "no listening
/// ports" from "this platform has no /proc").
fn proc_net_available() -> bool {
    std::fs::metadata("/proc/net/tcp").is_ok()
}

/// The set of TCP ports in the LISTEN state, from `/proc/net/tcp{,6}`.
fn listening_ports() -> HashSet<u16> {
    let mut ports = HashSet::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines().skip(1) {
            // Columns: sl  local_address  rem_address  st  ...
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 {
                continue;
            }
            // st == 0A is TCP_LISTEN.
            if cols[3] != "0A" {
                continue;
            }
            // local_address is HEX_IP:HEX_PORT.
            if let Some((_, port_hex)) = cols[1].rsplit_once(':') {
                if let Ok(port) = u16::from_str_radix(port_hex, 16) {
                    if port != 0 {
                        ports.insert(port);
                    }
                }
            }
        }
    }
    ports
}
