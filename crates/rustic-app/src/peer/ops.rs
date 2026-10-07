//! Operations behind the `lan_*` commands, host-generic so the desktop app
//! (Tauri commands) and rustic-server (its command table) share them. Peer
//! API calls go to `<peer>/lan/...` (see [`super::api_base`]).

use serde::{Deserialize, Serialize};

use super::client as cs;
use super::{consent, HostRef, LanState, Peer, Share};

/// Status of peer sync on this machine.
#[derive(Debug, Serialize)]
pub struct LanStatus {
    pub enabled: bool,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub port: u16,
    /// This machine's LAN IPv4, for "connect from another device".
    pub ip: Option<String>,
    /// `ip:port` other devices can enter in Add machine.
    pub address: Option<String>,
    /// Public Cloudflare tunnel URL, when one is running.
    pub tunnel_url: Option<String>,
    /// `"cloudflare"`, `"portforward"`, or `None` (LAN only).
    pub internet_mode: Option<String>,
    /// The listener's fixed port others forward to (for port-forwarding instructions).
    pub default_port: u16,
}

/// A device shown in the Cloud → Machines list.
#[derive(Debug, Serialize)]
pub struct LanDevice {
    pub device_id: String,
    /// Display name (nickname when set).
    pub name: String,
    /// The device's own name, before any nickname.
    pub device_name: String,
    pub nickname: Option<String>,
    pub addr: Option<String>,
    pub online: bool,
    pub paired: bool,
    /// Added by address rather than found via auto-discovery.
    pub manual: bool,
    /// Rustic version it last reported (`""` = an older build without one).
    pub version: Option<String>,
    /// Known to run a different version — everything is blocked until both match.
    pub update_required: bool,
    /// It forgot this machine (rejects our token) — pair again.
    pub needs_repair: bool,
}

/// One local metadata item (for the Push list and the sharing picker).
#[derive(Debug, Serialize)]
pub struct LocalMetaItem {
    pub key: String,
    pub category: String,
    pub name: String,
}

/// A project picked for a push / pull.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProject {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Pull only: parent folder for a project that doesn't exist here yet.
    #[serde(default)]
    pub target_parent: Option<String>,
}

/// Outcome of [`sync_items`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItemsResult {
    pub projects_ok: usize,
    pub failures: Vec<String>,
    pub meta: Option<crate::meta_sync::MetaApplySummary>,
}

/// Load (or create) the identity and record it in `lan`.
async fn load_identity(host: &HostRef, lan: &LanState) -> Result<super::Identity, String> {
    let dir = host.data_dir()?;
    let name = host.default_device_name();
    let id = tokio::task::spawn_blocking(move || super::load_or_create_identity_named(&dir, name))
        .await
        .map_err(|e| e.to_string())??;
    lan.lock().identity = Some(id.clone());
    Ok(id)
}

/// Serializes [`start`] / [`stop`] restarts: concurrent starts (startup
/// restore racing a toggle or rename) used to bind two listeners, the second
/// on a random port, and advertise whichever finished last.
static START_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes Cloudflare tunnel starts.
static TUNNEL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Start the TLS listener on port 47820 (+ fallback) and mDNS (idempotent).
/// Also restores the Cloudflare tunnel when that mode is on, watches for
/// network (IP) changes, and tells paired devices where we are now.
pub async fn start(host: &HostRef, lan: &LanState) -> Result<(), String> {
    let _guard = START_LOCK.lock().await;
    if lan.lock().identity.is_some() {
        return Ok(());
    }
    let id = load_identity(host, lan).await?;
    let port = match super::listener::start(host.clone(), lan.clone(), id.clone()).await {
        Ok(p) => p,
        Err(e) => {
            lan.lock().identity = None;
            return Err(e);
        }
    };
    match super::discovery::start(lan, &id, port) {
        Ok(daemon) => lan.lock().mdns = Some(daemon),
        Err(e) => tracing::warn!("LAN discovery unavailable: {e}"),
    }
    tokio::spawn(watch_network(host.clone(), lan.clone(), port));
    if host.data_dir().ok().and_then(|d| super::internet_mode(&d)).as_deref() == Some("cloudflare") {
        let (h, l) = (host.clone(), lan.clone());
        tokio::spawn(async move {
            if let Err(e) = start_tunnel(&h, &l).await {
                tracing::warn!("Cloudflare tunnel did not start: {e}");
            }
            announce(&h, &l).await;
        });
    }
    announce(host, lan).await;
    Ok(())
}

/// Every few seconds: if this machine's LAN IP changed (Wi-Fi switch, DHCP),
/// re-advertise over mDNS, refresh the UI and tell paired devices our new
/// address. Ends when the listener on `port` stops.
async fn watch_network(host: HostRef, lan: LanState, port: u16) {
    let mut last_ip = super::local_ipv4();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let id = {
            let inner = lan.lock();
            if inner.port != port || inner.shutdown.is_none() {
                return;
            }
            match inner.identity.clone() {
                Some(id) => id,
                None => return,
            }
        };
        let ip = super::local_ipv4();
        if ip == last_ip {
            continue;
        }
        tracing::info!(old = ?last_ip, new = ?ip, "peer: network address changed — re-announcing");
        last_ip = ip.clone();
        if let Some(d) = lan.lock().mdns.take() {
            let _ = d.shutdown();
        }
        {
            let mut inner = lan.lock();
            inner.discovered.clear();
            inner.working_addr.clear();
            // Nothing seen on the old network proves reachability on the new one.
            inner.seen.clear();
            inner.probed.clear();
        }
        crate::transfers::network_changed();
        host.emitter().emit_json("lan-peers-changed", serde_json::json!({}));
        if ip.is_some() {
            match super::discovery::start(&lan, &id, port) {
                Ok(daemon) => lan.lock().mdns = Some(daemon),
                Err(e) => tracing::warn!("LAN discovery restart failed: {e}"),
            }
        }
        host.emitter().emit_json("lan-status-changed", serde_json::json!({ "ip": ip }));
        // Give the new interface a moment to route before calling peers.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        announce(&host, &lan).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        host.emitter().emit_json("lan-peers-changed", serde_json::json!({}));
    }
}

/// Check in with every paired device (in the background) so each learns our
/// current address and we learn whether it's online, still paired with us,
/// and on the same version.
pub async fn announce(host: &HostRef, lan: &LanState) {
    let Ok(dir) = host.data_dir() else { return };
    for peer in super::load_peers(&dir) {
        tokio::spawn(hello_peer(host.clone(), lan.clone(), peer));
    }
}

/// Enable peer sync WITHOUT the TLS listener / mDNS — for a host that serves
/// [`super::listener::build_router`] on its own HTTP server (rustic-server).
/// Idempotent.
pub async fn start_identity_only(host: &HostRef, lan: &LanState) -> Result<(), String> {
    if lan.lock().identity.is_some() {
        return Ok(());
    }
    load_identity(host, lan).await.map(|_| ())
}

/// Stop the listener, mDNS and tunnel, and forget live state.
pub fn stop(lan: &LanState) {
    let mut inner = lan.lock();
    if let Some(tx) = inner.shutdown.take() {
        let _ = tx.send(());
    }
    if let Some(d) = inner.mdns.take() {
        let _ = d.shutdown();
    }
    inner.identity = None;
    inner.port = 0;
    inner.discovered.clear();
    inner.manual.clear();
    inner.seen.clear();
    inner.probed.clear();
    inner.pending_pairs.clear();
    inner.pending_transfers.clear();
    inner.tickets.clear();
    inner.working_addr.clear();
    inner.pair_by_device.clear();
    inner.transfer_by_device.clear();
    for (_, tx) in inner.outgoing.drain() {
        let _ = tx.send(true);
    }
    if let Some(t) = inner.tunnel.take() {
        t.stop();
    }
    super::set_public_url(None);
    super::set_listen_port(0);
}

/// Current status.
pub async fn status(host: &HostRef, lan: &LanState) -> Result<LanStatus, String> {
    let (enabled, device_id, device_name, port, tunnel_url) = {
        let mut inner = lan.lock();
        let tunnel_url = inner
            .tunnel
            .as_mut()
            .and_then(|t| t.alive().then(|| t.url.clone()));
        (
            inner.identity.is_some(),
            inner.identity.as_ref().map(|i| i.device_id.clone()),
            inner.identity.as_ref().map(|i| i.device_name.clone()),
            inner.port,
            tunnel_url,
        )
    };
    let ip = if enabled { super::local_ipv4() } else { None };
    Ok(LanStatus {
        enabled,
        device_id,
        device_name,
        port,
        address: ip.as_ref().filter(|_| port != 0).map(|ip| format!("{ip}:{port}")),
        ip,
        tunnel_url,
        internet_mode: host.data_dir().ok().and_then(|d| super::internet_mode(&d)),
        default_port: super::DEFAULT_PORT,
    })
}

/// Switch peer sync on/off (persisted across restarts) — full listener.
pub async fn set_enabled(host: &HostRef, lan: &LanState, enabled: bool) -> Result<(), String> {
    super::set_enabled_persisted(&host.data_dir()?, enabled)?;
    if enabled {
        start(host, lan).await
    } else {
        stop(lan);
        Ok(())
    }
}

/// Re-probe an offline paired device at most this often.
const PROBE_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// Addresses worth trying for a paired device, best first, de-duplicated.
pub fn candidate_addrs(lan: &LanState, peer: &Peer) -> Vec<String> {
    let inner = lan.lock();
    let mut out: Vec<String> = Vec::new();
    let mut push = |a: Option<String>| {
        if let Some(a) = a {
            if !out.contains(&a) {
                out.push(a);
            }
        }
    };
    push(inner.discovered.get(&peer.device_id).map(|d| d.addr.clone()));
    push(inner.seen.get(&peer.device_id).and_then(|s| s.addr.clone()));
    push(peer.addr.clone());
    push(peer.addr.as_deref().and_then(super::with_default_port));
    out
}

/// Check in with a paired device via `POST /lan/hello` (authenticated, so it
/// also proves it still knows us): tries each candidate address; on success
/// marks it online and remembers the address that worked. A 401 means it
/// forgot us (needs re-pairing); a 409 means a different version.
async fn hello_peer(host: HostRef, lan: LanState, peer: Peer) {
    for addr in candidate_addrs(&lan, &peer) {
        let Ok(client) = super::peer_client(&peer.fingerprint, &addr, &[]) else {
            continue;
        };
        let resp = client
            .post(format!("{}/hello", super::api_base(&addr)))
            .bearer_auth(&peer.token_out)
            .timeout(std::time::Duration::from_secs(4))
            .send()
            .await;
        let Ok(resp) = resp else { continue };
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        if let Some(v) = body.get("version").and_then(|v| v.as_str()) {
            lan.lock().versions.insert(peer.device_id.clone(), v.to_string());
        }
        if status == reqwest::StatusCode::CONFLICT {
            tracing::info!(device = %peer.device_id, theirs = ?body.get("version"), "peer: paired device runs a different version");
            if body.get("version").is_none() {
                lan.lock().versions.insert(peer.device_id.clone(), String::new());
            }
            lan.lock().mark_seen(&peer.device_id, Some(addr.clone()));
            return;
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            tracing::info!(device = %peer.device_id, "peer: paired device no longer recognises us — needs re-pairing");
            let mut inner = lan.lock();
            inner.needs_repair.insert(peer.device_id.clone());
            inner.mark_seen(&peer.device_id, Some(addr.clone()));
            return;
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            // Older build without /hello: still a different version.
            lan.lock().versions.insert(peer.device_id.clone(), String::new());
            lan.lock().mark_seen(&peer.device_id, Some(addr.clone()));
            return;
        }
        if !status.is_success() {
            continue;
        }
        remember_working(&host, &lan, &peer, &addr);
        lan.lock().needs_repair.remove(&peer.device_id);
        return;
    }
}

/// Record `addr` as where `peer` answered (live state + saved address).
fn remember_working(host: &HostRef, lan: &LanState, peer: &Peer, addr: &str) {
    crate::transfers::peer_reachable(&peer.display_name());
    {
        let mut inner = lan.lock();
        inner.mark_seen(&peer.device_id, Some(addr.to_string()));
        inner
            .working_addr
            .insert(peer.device_id.clone(), (addr.to_string(), std::time::Instant::now()));
    }
    if peer.addr.as_deref() != Some(addr) {
        if let Ok(dir) = host.data_dir() {
            let mut peers = super::load_peers(&dir);
            if let Some(p) = peers.iter_mut().find(|p| p.device_id == peer.device_id) {
                p.addr = Some(addr.to_string());
                let _ = super::save_peers(&dir, &peers);
            }
        }
    }
}

/// An address where `peer` answers right now: the one that worked within
/// [`super::ONLINE_WINDOW`], else the first candidate whose `/lan/info`
/// answers as this device (4s each). Refuses a different version.
pub async fn resolve_addr(host: &HostRef, lan: &LanState, peer: &Peer) -> Result<String, String> {
    if let Some(theirs) = lan.lock().versions.get(&peer.device_id).cloned() {
        if theirs != super::app_version() {
            // Re-check below: they may have updated since.
            lan.lock().working_addr.remove(&peer.device_id);
        }
    }
    let fresh = lan
        .lock()
        .working_addr
        .get(&peer.device_id)
        .filter(|(_, at)| at.elapsed() < super::ONLINE_WINDOW)
        .map(|(a, _)| a.clone());
    if let Some(a) = fresh {
        return Ok(a);
    }
    let candidates = candidate_addrs(lan, peer);
    for addr in &candidates {
        let Ok(client) = super::peer_client(&peer.fingerprint, addr, &[]) else {
            continue;
        };
        let resp = client
            .get(format!("{}/lan/info", super::base_url(addr)))
            .timeout(std::time::Duration::from_secs(4))
            .send()
            .await;
        let body: serde_json::Value = match resp {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(e) => {
                tracing::debug!(device = %peer.device_id, addr, "peer: address not answering: {e}");
                continue;
            }
        };
        if body.get("device_id").and_then(|v| v.as_str()) != Some(peer.device_id.as_str()) {
            continue;
        }
        let theirs = body.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
        lan.lock().versions.insert(peer.device_id.clone(), theirs.clone());
        if theirs != super::app_version() {
            lan.lock().mark_seen(&peer.device_id, Some(addr.clone()));
            return Err(super::version_mismatch_message(&peer.display_name(), Some(&theirs)));
        }
        remember_working(host, lan, peer, addr);
        return Ok(addr.clone());
    }
    tracing::warn!(device = %peer.device_id, tried = ?candidates, "peer: paired device unreachable");
    Err(if candidates.is_empty() {
        format!("{} has no known address yet — open Rustic there, or add it by address.", peer.display_name())
    } else {
        format!(
            "Couldn't reach {} (tried {}). Check it's running with sync on and on the same network, or connect via its tunnel URL.",
            peer.display_name(),
            candidates.join(", ")
        )
    })
}

/// Turn a peer's error response into a clear message, recording re-pair /
/// version state. Passes successful responses through.
pub async fn check_response(
    lan: &LanState,
    peer: &Peer,
    resp: reqwest::Response,
) -> Result<reqwest::Response, String> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let msg = body.get("error").and_then(|v| v.as_str()).map(str::to_string);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        lan.lock().needs_repair.insert(peer.device_id.clone());
        lan.lock().working_addr.remove(&peer.device_id);
        return Err(format!(
            "{} no longer recognises this machine (it was forgotten there). Pair again.",
            peer.display_name()
        ));
    }
    if status == reqwest::StatusCode::CONFLICT && body.get("code").and_then(|v| v.as_str()) == Some(super::VERSION_MISMATCH) {
        let theirs = body.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
        lan.lock().versions.insert(peer.device_id.clone(), theirs.clone());
        return Err(super::version_mismatch_message(&peer.display_name(), Some(&theirs)));
    }
    Err(msg.unwrap_or_else(|| format!("{} refused the request (HTTP {status})", peer.display_name())))
}

/// The paired device `device_id`.
pub fn paired(host: &HostRef, device_id: &str) -> Result<Peer, String> {
    super::load_peers(&host.data_dir()?)
        .into_iter()
        .find(|p| p.device_id == device_id)
        .ok_or_else(|| "That device is not paired — pair it first".to_string())
}

/// Whether `device_id` is blocked (different version or needs re-pairing).
fn blocked_reason(lan: &LanState, peer: &Peer) -> Option<String> {
    let inner = lan.lock();
    if inner.needs_repair.contains(&peer.device_id) {
        return Some(format!(
            "{} no longer recognises this machine. Pair again.",
            peer.display_name()
        ));
    }
    inner
        .versions
        .get(&peer.device_id)
        .filter(|v| v.as_str() != super::app_version())
        .map(|v| super::version_mismatch_message(&peer.display_name(), Some(v)))
}

/// Discovered, manually added and paired devices. A paired device is online
/// when mDNS sees it, it recently called us, or a background probe of its
/// saved address succeeded.
pub async fn devices(host: &HostRef, lan: &LanState) -> Result<Vec<LanDevice>, String> {
    let peers = super::load_peers(&host.data_dir()?);
    let (discovered, manual, running) = {
        let inner = lan.lock();
        (
            inner.discovered.clone(),
            inner.manual.clone(),
            inner.identity.is_some(),
        )
    };
    let mut out: Vec<LanDevice> = Vec::new();
    for p in &peers {
        let found = discovered.get(&p.device_id);
        let (seen_recently, seen_addr, version, needs_repair) = {
            let inner = lan.lock();
            (
                inner.recently_seen(&p.device_id),
                inner.seen.get(&p.device_id).and_then(|s| s.addr.clone()),
                inner
                    .versions
                    .get(&p.device_id)
                    .cloned()
                    .or_else(|| found.and_then(|d| d.version.clone())),
                inner.needs_repair.contains(&p.device_id),
            )
        };
        let online = running && (found.is_some() || seen_recently);
        if running {
            let due = {
                let mut inner = lan.lock();
                let every = if online { super::ONLINE_WINDOW / 2 } else { PROBE_EVERY };
                let due = inner
                    .probed
                    .get(&p.device_id)
                    .is_none_or(|t| t.elapsed() >= every);
                if due {
                    inner
                        .probed
                        .insert(p.device_id.clone(), std::time::Instant::now());
                }
                due
            };
            if due {
                tokio::spawn(hello_peer(host.clone(), lan.clone(), p.clone()));
            }
        }
        out.push(LanDevice {
            device_id: p.device_id.clone(),
            name: p.display_name(),
            device_name: p.name.clone(),
            nickname: p.nickname.clone(),
            addr: found.map(|d| d.addr.clone()).or(seen_addr).or(p.addr.clone()),
            online,
            paired: true,
            manual: false,
            update_required: version.as_deref().is_some_and(|v| v != super::app_version()),
            version,
            needs_repair,
        });
    }
    let own_ip = super::local_ipv4();
    for (src, manual_flag) in [(&discovered, false), (&manual, true)] {
        for d in src.values() {
            if out.iter().any(|o| o.device_id == d.device_id) {
                continue;
            }
            // Another Rustic install on this same computer (e.g. the dev and
            // release builds have separate identities) — not a real peer.
            // Still reachable via Add machine if someone wants it on purpose.
            if !manual_flag && super::is_this_host(&d.addr, own_ip.as_deref()) {
                continue;
            }
            out.push(LanDevice {
                device_id: d.device_id.clone(),
                name: d.name.clone(),
                device_name: d.name.clone(),
                nickname: None,
                addr: Some(d.addr.clone()),
                online: true,
                paired: false,
                manual: manual_flag,
                update_required: d.version.as_deref().is_some_and(|v| v != super::app_version()),
                version: d.version.clone(),
                needs_repair: false,
            });
        }
    }
    out.sort_by(|a, b| {
        (!a.paired, !a.online, a.name.to_lowercase())
            .cmp(&(!b.paired, !b.online, b.name.to_lowercase()))
    });
    Ok(out)
}

/// "Add machine" by address: read the device's identity from `/lan/info`
/// (capturing its certificate on a pinned LAN address) so it can be paired
/// like a discovered one.
pub async fn add_manual(host: &HostRef, lan: &LanState, address: &str) -> Result<LanDevice, String> {
    let me = lan
        .lock()
        .identity
        .clone()
        .ok_or("Turn on local-network sync first")?;
    let addr = super::normalize_addr(address)?;
    let url_addr = super::is_url_addr(&addr);
    let (client, captured) = if url_addr {
        (
            super::peer_client("", &addr, &[])?,
            std::sync::Arc::new(std::sync::Mutex::new(None)),
        )
    } else {
        super::capture_client()?
    };
    let resp = client
        .get(format!("{}/lan/info", super::base_url(&addr)))
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| {
            if url_addr {
                format!("Could not reach {addr}: {e}. Check the tunnel / server is running on the other machine.")
            } else {
                format!(
                    "Could not reach {addr}: {e}. Check that Rustic is running there with local-network sync on, and that its firewall (or router port forward) allows port {}.",
                    super::DEFAULT_PORT
                )
            }
        })?;
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| format!("{addr} is not a Rustic device"))?;
    let claimed_fp = body.get("fingerprint").and_then(|v| v.as_str()).map(str::to_string);
    // Over a URL address the TLS certificate (if any) isn't the device's own,
    // so its fingerprint can only come from its identity response; the
    // matching 6-digit pairing code on both screens is what authenticates it.
    let fp = if url_addr {
        claimed_fp.clone().ok_or(format!("{addr} is not a Rustic device"))?
    } else {
        captured
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or("No certificate was presented")?
    };
    let device_id = body
        .get("device_id")
        .and_then(|v| v.as_str())
        .ok_or(format!("{addr} is not a Rustic device"))?
        .to_string();
    if claimed_fp.as_deref() != Some(fp.as_str()) || !fp.starts_with(&device_id) {
        return Err(format!("{addr} presented a certificate that doesn't match its identity"));
    }
    if device_id == me.device_id {
        return Err("That address is this machine".into());
    }
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Rustic")
        .to_string();
    let version = body.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    lan.lock().versions.insert(device_id.clone(), version.clone());
    let update_required = version != super::app_version();
    tracing::info!(addr, device = %device_id, version, "peer: added machine by address");

    let dir = host.data_dir()?;
    let mut peers = super::load_peers(&dir);
    if let Some(p) = peers.iter_mut().find(|p| p.device_id == device_id) {
        if p.fingerprint != fp {
            return Err("That device's certificate changed since you paired — forget it and pair again".into());
        }
        p.addr = Some(addr.clone());
        let out = LanDevice {
            device_id: p.device_id.clone(),
            name: p.display_name(),
            device_name: p.name.clone(),
            nickname: p.nickname.clone(),
            addr: Some(addr.clone()),
            online: true,
            paired: true,
            manual: false,
            version: Some(version.clone()),
            update_required,
            needs_repair: lan.lock().needs_repair.contains(&device_id),
        };
        super::save_peers(&dir, &peers)?;
        lan.lock().mark_seen(&device_id, Some(addr));
        return Ok(out);
    }
    lan.lock().manual.insert(
        device_id.clone(),
        super::Discovered {
            device_id: device_id.clone(),
            name: name.clone(),
            fingerprint: fp,
            addr: addr.clone(),
            version: Some(version.clone()),
            fullname: String::new(),
        },
    );
    Ok(LanDevice {
        device_id,
        name: name.clone(),
        device_name: name,
        nickname: None,
        addr: Some(addr),
        online: true,
        paired: false,
        manual: true,
        version: Some(version),
        update_required,
        needs_repair: false,
    })
}

/// Set (or clear, when empty) the local nickname for a paired device.
pub fn rename(host: &HostRef, device_id: &str, nickname: &str) -> Result<(), String> {
    let dir = host.data_dir()?;
    let mut peers = super::load_peers(&dir);
    let p = peers
        .iter_mut()
        .find(|p| p.device_id == device_id)
        .ok_or("That device is not paired")?;
    let n = nickname.trim();
    p.nickname = (!n.is_empty()).then(|| n.to_string());
    super::save_peers(&dir, &peers)
}

/// Rename this machine (what other devices see). Restarts the listener and
/// mDNS advertisement if running so the new name is announced; an
/// identity-only host just reloads its identity.
pub async fn set_device_name(host: &HostRef, lan: &LanState, name: &str) -> Result<(), String> {
    super::set_custom_device_name(&host.data_dir()?, name)?;
    let (running, listener) = {
        let inner = lan.lock();
        (inner.identity.is_some(), inner.port != 0)
    };
    if running && listener {
        stop(lan);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        start(host, lan).await?;
    } else if running {
        load_identity(host, lan).await?;
    }
    Ok(())
}

/// The code this machine shows while pairing with `device_id`.
pub fn pair_code(lan: &LanState, device_id: &str) -> Result<String, String> {
    let inner = lan.lock();
    let me = inner
        .identity
        .as_ref()
        .ok_or("Turn on local-network sync first")?;
    if let Some(d) = inner.candidate(device_id) {
        return Ok(super::pairing_code(&me.fingerprint, &d.fingerprint));
    }
    drop(inner);
    Err("That device is not on the network right now".into())
}

/// The code shown while (re-)pairing a device we already know (paired peers
/// aren't kept in the candidate lists).
pub fn pair_code_for(host: &HostRef, lan: &LanState, device_id: &str) -> Result<String, String> {
    pair_code(lan, device_id).or_else(|e| {
        let me = lan.lock().identity.clone().ok_or("Turn on local-network sync first")?;
        let peer = paired(host, device_id).map_err(|_| e)?;
        Ok(super::pairing_code(&me.fingerprint, &peer.fingerprint))
    })
}

/// Who to send a pairing request to: a discovered / manually added device,
/// else (re-pairing) an already-paired one at an address that answers.
async fn pair_target(host: &HostRef, lan: &LanState, device_id: &str) -> Result<super::Discovered, String> {
    if let Some(d) = lan.lock().candidate(device_id) {
        return Ok(d);
    }
    let peer = paired(host, device_id).map_err(|_| "That device is not on the network right now".to_string())?;
    // Version is checked again right before the request; don't fail here.
    let addr = match resolve_addr(host, lan, &peer).await {
        Ok(a) => a,
        Err(e) => {
            let known = lan.lock().versions.get(device_id).cloned();
            match (known, candidate_addrs(lan, &peer).into_iter().next()) {
                (Some(_), Some(a)) => a,
                _ => return Err(e),
            }
        }
    };
    Ok(super::Discovered {
        device_id: peer.device_id.clone(),
        name: peer.name.clone(),
        fingerprint: peer.fingerprint.clone(),
        addr,
        version: lan.lock().versions.get(device_id).cloned(),
        fullname: String::new(),
    })
}

/// Register a cancellable outgoing wait for `device_id`; the receiver turns
/// `true` when [`cancel_outgoing`] is called.
fn begin_outgoing(lan: &LanState, device_id: &str) -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    if let Some(old) = lan.lock().outgoing.insert(device_id.to_string(), tx) {
        let _ = old.send(true);
    }
    rx
}

/// Drop the outgoing-wait registration for `device_id`.
fn end_outgoing(lan: &LanState, device_id: &str) {
    lan.lock().outgoing.remove(device_id);
}

/// Resolves when `rx` turns `true`.
async fn cancelled(mut rx: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Stop waiting on our pairing / approval request to `device_id`; the other
/// machine's prompt is dismissed too.
pub fn cancel_outgoing(lan: &LanState, device_id: &str) -> Result<(), String> {
    let tx = lan.lock().outgoing.remove(device_id);
    match tx {
        Some(tx) => {
            let _ = tx.send(true);
            Ok(())
        }
        None => Ok(()),
    }
}

/// Ask `device_id` to pair (or re-pair). Checks the version first, then waits
/// for the other machine's Accept/Decline; cancellable via [`cancel_outgoing`].
/// Returns the other device's name.
pub async fn pair(host: &HostRef, lan: &LanState, device_id: &str) -> Result<String, String> {
    let me = lan
        .lock()
        .identity
        .clone()
        .ok_or("Turn on local-network sync first")?;
    let target = pair_target(host, lan, device_id).await?;
    let client = super::peer_client(&target.fingerprint, &target.addr, &[])?;
    // Version gate before bothering the other user.
    if let Ok(r) = client
        .get(format!("{}/lan/info", super::base_url(&target.addr)))
        .timeout(std::time::Duration::from_secs(6))
        .send()
        .await
    {
        let body: serde_json::Value = r.json().await.unwrap_or_default();
        let theirs = body.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
        lan.lock().versions.insert(target.device_id.clone(), theirs.clone());
        if theirs != super::app_version() {
            return Err(super::version_mismatch_message(&target.name, Some(&theirs)));
        }
    }
    tracing::info!(device = %target.device_id, addr = %target.addr, "peer: sending pairing request");
    let token_in = super::random_token()?;
    let cancel = begin_outgoing(lan, device_id);
    let send = client
        .post(format!("{}/lan/pair", super::base_url(&target.addr)))
        .timeout(std::time::Duration::from_secs(100))
        .json(&serde_json::json!({
            "device_id": me.device_id,
            "name": me.device_name,
            "fingerprint": me.fingerprint,
            "token": token_in,
        }))
        .send();
    let resp = tokio::select! {
        r = send => r,
        _ = cancelled(cancel) => {
            end_outgoing(lan, device_id);
            tracing::info!(device = %target.device_id, "peer: pairing request cancelled by the user");
            let _ = client
                .post(format!("{}/lan/pair/cancel", super::base_url(&target.addr)))
                .timeout(std::time::Duration::from_secs(5))
                .json(&serde_json::json!({ "device_id": me.device_id }))
                .send()
                .await;
            return Err("Pairing cancelled".into());
        }
    };
    end_outgoing(lan, device_id);
    let resp = resp.map_err(|e| {
        tracing::warn!(device = %target.device_id, addr = %target.addr, "peer: pairing request failed: {e}");
        format!("Could not reach {}: {e}", target.name)
    })?;
    if resp.status() == reqwest::StatusCode::CONFLICT {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let theirs = body.get("version").and_then(|v| v.as_str()).map(str::to_string);
        return Err(super::version_mismatch_message(&target.name, theirs.as_deref()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !body
        .get("accepted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(format!(
            "{} declined the pairing request (or it timed out)",
            target.name
        ));
    }
    let token_out = body
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or("pairing response carried no token")?;
    let dir = host.data_dir()?;
    let existing = super::load_peers(&dir)
        .into_iter()
        .find(|p| p.device_id == target.device_id);
    let nickname = existing.as_ref().and_then(|p| p.nickname.clone());
    let meta_view = existing.as_ref().is_some_and(|p| p.meta_view);
    let share = existing.map(|p| p.share).unwrap_or_default();
    super::upsert_peer(
        &dir,
        Peer {
            device_id: target.device_id.clone(),
            name: target.name.clone(),
            fingerprint: target.fingerprint.clone(),
            token_out: token_out.to_string(),
            token_in,
            addr: Some(target.addr.clone()),
            nickname,
            share,
            meta_view,
        },
    )?;
    {
        let mut inner = lan.lock();
        inner.manual.remove(&target.device_id);
        inner.needs_repair.remove(&target.device_id);
        inner.mark_seen(&target.device_id, Some(target.addr.clone()));
        inner
            .working_addr
            .insert(target.device_id.clone(), (target.addr.clone(), std::time::Instant::now()));
    }
    tracing::info!(device = %target.device_id, "peer: paired");
    Ok(target.name)
}

/// Answer an incoming pairing request.
pub fn respond_pair(lan: &LanState, request_id: &str, accept: bool) -> Result<(), String> {
    let tx = lan
        .lock()
        .pending_pairs
        .remove(request_id)
        .ok_or("That pairing request has expired")?;
    let _ = tx.send(accept);
    Ok(())
}

/// Forget a paired device (it must pair again to sync). Tells the device
/// first (best effort) so it forgets us too instead of keeping a dead pairing.
pub async fn forget(host: &HostRef, lan: &LanState, device_id: &str) -> Result<(), String> {
    let dir = host.data_dir()?;
    if let Ok(peer) = paired(host, device_id) {
        let addr = candidate_addrs(lan, &peer).into_iter().next();
        if let Some(addr) = addr {
            if let Ok(client) = super::peer_client(&peer.fingerprint, &addr, &[]) {
                let r = client
                    .post(format!("{}/unpair", super::api_base(&addr)))
                    .bearer_auth(&peer.token_out)
                    .timeout(std::time::Duration::from_secs(4))
                    .send()
                    .await;
                tracing::info!(device = %device_id, notified = r.as_ref().is_ok_and(|r| r.status().is_success()), "peer: forgetting device");
            }
        }
    }
    let mut peers = super::load_peers(&dir);
    peers.retain(|p| p.device_id != device_id);
    super::save_peers(&dir, &peers)?;
    let mut inner = lan.lock();
    inner.needs_repair.remove(device_id);
    inner.working_addr.remove(device_id);
    inner.seen.remove(device_id);
    Ok(())
}

/// Client + peer API base (`<peer>/lan`) + token for a paired device at an
/// address that answers right now, sending `extra` headers. Refuses devices
/// on another version or that need re-pairing.
pub async fn connect_with(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    extra: &[(&'static str, String)],
) -> Result<(reqwest::Client, String, String), String> {
    let peer = paired(host, device_id)?;
    if let Some(reason) = blocked_reason(lan, &peer) {
        // Re-check: they may have updated / re-paired since.
        hello_peer(host.clone(), lan.clone(), peer.clone()).await;
        if let Some(reason2) = blocked_reason(lan, &peer) {
            let _ = reason;
            return Err(reason2);
        }
    }
    let addr = resolve_addr(host, lan, &peer).await?;
    let client = super::peer_client(&peer.fingerprint, &addr, extra)?;
    Ok((client, super::api_base(&addr), peer.token_out))
}

/// Client + peer API base + token for a paired device.
pub async fn connect(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
) -> Result<(reqwest::Client, String, String), String> {
    connect_with(host, lan, device_id, &[]).await
}

/// Push to a paired device: everything, or one project.
pub async fn push(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    project_id: Option<String>,
) -> Result<String, String> {
    let (client, base, token) = connect(host, lan, device_id).await?;
    let rep = cs::reporter(host, "push");
    match project_id {
        Some(pid) => cs::push_project_env(host, &client, &base, &token, pid, &rep, cs::LAN_STREAMS).await,
        None => cs::push_env(host, &client, &base, &token, &rep, cs::LAN_STREAMS).await,
    }
}

/// Pull from a paired device: everything, or one project (optionally into a chosen folder).
pub async fn pull(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    project_id: Option<String>,
    target_parent: Option<String>,
) -> Result<String, String> {
    let (client, base, token) = connect(host, lan, device_id).await?;
    let rep = cs::reporter(host, "pull");
    match project_id {
        Some(pid) => {
            cs::pull_project_env(host, &client, &base, &token, pid, target_parent, &rep, cs::LAN_STREAMS)
                .await
        }
        None => cs::pull_env(host, &client, &base, &token, &rep, cs::LAN_STREAMS).await,
    }
}

/// Projects a paired device shares with us.
pub async fn list_projects(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
) -> Result<Vec<cs::RemoteProject>, String> {
    let (client, base, token) = connect(host, lan, device_id).await?;
    cs::list_projects_env(&client, &base, &token).await
}

/// Metadata diff against a paired device.
pub async fn meta_preview(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    direction: &str,
) -> Result<Vec<crate::meta_sync::MetaDiffEntry>, String> {
    let (client, base, token) = connect(host, lan, device_id).await?;
    cs::meta_preview_env(host, &client, &base, &token, direction).await
}

/// Metadata merge with a paired device.
pub async fn meta_apply(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    direction: &str,
    overwrite: Vec<String>,
) -> Result<crate::meta_sync::MetaApplySummary, String> {
    let (client, base, token) = connect(host, lan, device_id).await?;
    cs::meta_apply_env(host, &client, &base, &token, direction, overwrite, None).await
}

/// This machine's metadata items, without their content.
pub async fn local_meta(host: &HostRef) -> Result<Vec<LocalMetaItem>, String> {
    let bundle = cs::local_meta(host).await?;
    let mut out: Vec<LocalMetaItem> = bundle
        .items
        .iter()
        .map(|i| LocalMetaItem {
            key: i.key(),
            category: i.category.clone(),
            name: i.name.clone(),
        })
        .collect();
    out.sort_by(|a, b| (a.category.as_str(), a.name.as_str()).cmp(&(b.category.as_str(), b.name.as_str())));
    Ok(out)
}

/// What this machine shares with a paired device.
pub fn get_share(host: &HostRef, device_id: &str) -> Result<Share, String> {
    super::load_peers(&host.data_dir()?)
        .into_iter()
        .find(|p| p.device_id == device_id)
        .map(|p| p.share)
        .ok_or_else(|| "That device is not paired".to_string())
}

/// Replace what this machine shares with a paired device.
pub fn set_share(host: &HostRef, device_id: &str, share: Share) -> Result<(), String> {
    let dir = host.data_dir()?;
    let mut peers = super::load_peers(&dir);
    let p = peers
        .iter_mut()
        .find(|p| p.device_id == device_id)
        .ok_or("That device is not paired")?;
    p.share = share;
    super::save_peers(&dir, &peers)
}

/// Answer an incoming push / pull approval prompt.
pub fn respond_transfer(lan: &LanState, request_id: &str, accept: bool) -> Result<(), String> {
    let tx = lan
        .lock()
        .pending_transfers
        .remove(request_id)
        .ok_or("That request has expired")?;
    let _ = tx.send(accept);
    Ok(())
}

/// Ask paired device `device_id` to approve `request` (its user sees the
/// exact list). Cancellable via [`cancel_outgoing`], which also dismisses the
/// prompt there. Returns the approval ticket.
pub async fn request_approval(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    request: &consent::TransferRequest,
) -> Result<String, String> {
    let peer = paired(host, device_id)?;
    let (client, base, token) = connect(host, lan, device_id).await?;
    tracing::info!(device = %device_id, kind = %request.kind, "peer: asking for approval");
    let cancel = begin_outgoing(lan, device_id);
    let send = client
        .post(format!("{base}/request"))
        .bearer_auth(&token)
        .timeout(consent::PROMPT_TIMEOUT + std::time::Duration::from_secs(10))
        .json(request)
        .send();
    let resp = tokio::select! {
        r = send => r,
        _ = cancelled(cancel) => {
            end_outgoing(lan, device_id);
            let _ = client
                .post(format!("{base}/request/cancel"))
                .bearer_auth(&token)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await;
            tracing::info!(device = %device_id, "peer: approval request cancelled by the user");
            return Err("Request cancelled".into());
        }
    };
    end_outgoing(lan, device_id);
    let resp = resp.map_err(|e| {
        tracing::warn!(device = %device_id, "peer: approval request failed: {e}");
        lan.lock().working_addr.remove(device_id);
        format!("Could not reach {}: {e}", peer.display_name())
    })?;
    let body: serde_json::Value = check_response(lan, &peer, resp)
        .await?
        .json()
        .await
        .unwrap_or_default();
    body.get("approved")
        .and_then(|v| v.as_bool())
        .filter(|a| *a)
        .and_then(|_| body.get("ticket").and_then(|v| v.as_str()))
        .map(str::to_string)
        .ok_or_else(|| format!("{} declined (or didn't answer in time)", peer.display_name()))
}

/// Push or pull the picked projects and metadata items with a paired device.
/// First asks the other machine to approve the exact list (the receiver for a
/// push, the sender for a pull); nothing moves until it does. Received
/// projects replace the local copy; picked metadata items overwrite
/// same-name items.
pub async fn sync_items(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    direction: &str,
    projects: Vec<SyncProject>,
    meta: Vec<consent::RequestedMeta>,
) -> Result<SyncItemsResult, String> {
    if direction != "push" && direction != "pull" {
        return Err("direction must be push or pull".into());
    }
    let peer_name = super::load_peers(&host.data_dir()?)
        .into_iter()
        .find(|p| p.device_id == device_id)
        .map(|p| p.display_name())
        .unwrap_or_else(|| "the other machine".into());
    let request = consent::TransferRequest {
        kind: direction.to_string(),
        projects: projects
            .iter()
            .map(|p| consent::RequestedProject { id: p.id.clone(), name: p.name.clone(), exists: false })
            .collect(),
        meta: meta.clone(),
        ..Default::default()
    };
    let ticket = request_approval(host, lan, device_id, &request).await?;

    let mut out = SyncItemsResult { projects_ok: 0, failures: Vec::new(), meta: None };
    for p in projects {
        let label = if p.name.is_empty() { p.id.clone() } else { p.name.clone() };
        let handle = crate::transfers::begin(host.emitter(), direction, &label, &peer_name, 0, 0);
        let rep = crate::cloud_sync::SyncReporter::with_transfer(direction, host.emitter(), handle.clone());
        let res = async {
            let (c, b, t) = connect_with(
                host,
                lan,
                device_id,
                &[(super::TICKET_HEADER, ticket.clone()), (super::PROJECT_HEADER, p.id.clone())],
            )
            .await?;
            if direction == "push" {
                cs::push_project_env(host, &c, &b, &t, p.id.clone(), &rep, cs::LAN_STREAMS).await
            } else {
                cs::pull_project_env(host, &c, &b, &t, p.id.clone(), p.target_parent.clone(), &rep, cs::LAN_STREAMS).await
            }
        }
        .await;
        handle.finish(res.as_ref().map(|_| None).map_err(|e| e.clone()));
        match res {
            Ok(_) => out.projects_ok += 1,
            Err(e) => out.failures.push(format!("{label}: {e}")),
        }
    }

    if !meta.is_empty() {
        let keys: std::collections::HashSet<String> = meta.iter().map(|m| m.key.clone()).collect();
        let (c, b, t) = connect_with(host, lan, device_id, &[(super::TICKET_HEADER, ticket.clone())]).await?;
        let res: Result<crate::meta_sync::MetaApplySummary, String> = if direction == "push" {
            let mut bundle = cs::local_meta(host).await?;
            bundle.items.retain(|i| keys.contains(&i.key()));
            let r = c
                .post(format!("{b}/api/sync/meta"))
                .bearer_auth(&t)
                .json(&serde_json::json!({ "bundle": bundle, "overwrite": keys }))
                .send()
                .await
                .map_err(|e| format!("Sending metadata failed: {e}"))?;
            if r.status().is_success() {
                r.json().await.map_err(|e| e.to_string())
            } else {
                Err(format!("{peer_name} refused the metadata (HTTP {})", r.status()))
            }
        } else {
            let r = c
                .get(format!("{b}/api/sync/meta"))
                .bearer_auth(&t)
                .send()
                .await
                .map_err(|e| format!("Fetching metadata failed: {e}"))?;
            if !r.status().is_success() {
                Err(format!("{peer_name} refused the metadata (HTTP {})", r.status()))
            } else {
                let mut bundle: crate::meta_sync::MetaBundle = r.json().await.map_err(|e| e.to_string())?;
                bundle.items.retain(|i| keys.contains(&i.key()));
                cs::apply_local_meta(host, bundle, keys).await
            }
        };
        match res {
            Ok(s) => out.meta = Some(s),
            Err(e) => out.failures.push(format!("metadata: {e}")),
        }
    }
    Ok(out)
}

/// Start (or reuse) the Cloudflare quick tunnel, downloading cloudflared
/// first when it isn't installed. Returns the public URL. Needs the TLS
/// listener ([`start`]).
pub async fn start_tunnel(host: &HostRef, lan: &LanState) -> Result<String, String> {
    let _guard = TUNNEL_LOCK.lock().await;
    let port = {
        let mut inner = lan.lock();
        if inner.identity.is_none() || inner.port == 0 {
            return Err("Turn on local network sync first".into());
        }
        if let Some(t) = inner.tunnel.as_mut() {
            if t.alive() {
                return Ok(t.url.clone());
            }
        }
        inner.tunnel = None;
        inner.port
    };
    let dir = host.data_dir()?;
    let emitter = host.emitter();
    let bin = super::tunnel::ensure_binary(&dir, move |done, total| {
        emitter.emit_json(
            "lan-cloudflared-download",
            serde_json::json!({ "done": done, "total": total }),
        );
    })
    .await?;
    let t = super::tunnel::start(port, &bin).await?;
    let url = t.url.clone();
    lan.lock().tunnel = Some(t);
    super::set_public_url(Some(url.clone()));
    tracing::info!(url, port, "peer: Cloudflare tunnel up");
    host.emitter().emit_json("lan-status-changed", serde_json::json!({ "tunnel_url": url }));
    Ok(url)
}

/// Stop the tunnel, if any.
pub fn stop_tunnel(lan: &LanState) {
    if let Some(t) = lan.lock().tunnel.take() {
        t.stop();
    }
    super::set_public_url(None);
}

/// How other machines reach this one from outside the LAN: `"cloudflare"`,
/// `"portforward"` or `"off"`. Remembered (see [`restore`]). Returns the
/// tunnel URL in Cloudflare mode.
pub async fn set_internet_mode(host: &HostRef, lan: &LanState, mode: &str) -> Result<Option<String>, String> {
    let dir = host.data_dir()?;
    match mode {
        "cloudflare" => {
            let url = start_tunnel(host, lan).await?;
            super::set_internet_mode(&dir, Some("cloudflare"))?;
            Ok(Some(url))
        }
        "portforward" => {
            stop_tunnel(lan);
            super::set_internet_mode(&dir, Some("portforward"))?;
            Ok(None)
        }
        "off" => {
            stop_tunnel(lan);
            super::set_internet_mode(&dir, None)?;
            Ok(None)
        }
        other => Err(format!("unknown mode: {other}")),
    }
}

/// Startup: restore the listener if it was on ([`start`] also brings back the
/// Cloudflare tunnel when that mode is on).
pub async fn restore(host: &HostRef, lan: &LanState) {
    let Ok(dir) = host.data_dir() else { return };
    if !super::is_enabled_persisted(&dir) {
        return;
    }
    if let Err(e) = start(host, lan).await {
        tracing::warn!("LAN sync did not start: {e}");
    }
}


/// POST `body` to `<peer>/lan<path>` on a paired device and decode JSON.
async fn peer_post<T: serde::de::DeserializeOwned>(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    path: &str,
    body: serde_json::Value,
) -> Result<T, String> {
    let peer = paired(host, device_id)?;
    let (client, base, token) = connect(host, lan, device_id).await?;
    let resp = client
        .post(format!("{base}{path}"))
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            lan.lock().working_addr.remove(device_id);
            format!("Could not reach {}: {e}", peer.display_name())
        })?;
    check_response(lan, &peer, resp)
        .await?
        .json()
        .await
        .map_err(|e| e.to_string())
}

/// One folder of a project a paired device shares with us.
pub async fn list_files(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    project_id: &str,
    path: &str,
) -> Result<Vec<super::files::FsEntry>, String> {
    peer_post(host, lan, device_id, "/api/fs/list", serde_json::json!({ "project_id": project_id, "path": path })).await
}

/// One file of a shared project, for the preview pane.
pub async fn preview_file(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    project_id: &str,
    path: &str,
) -> Result<super::files::Preview, String> {
    peer_post(host, lan, device_id, "/api/fs/preview", serde_json::json!({ "project_id": project_id, "path": path })).await
}

/// Total size of a selection on a paired device.
pub async fn remote_size(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    items: &[consent::RequestedFile],
) -> Result<super::files::SizeInfo, String> {
    peer_post(host, lan, device_id, "/api/fs/size", serde_json::json!({ "items": items })).await
}

/// Which of `names` already exist in `dir` of a paired device's project.
pub async fn remote_conflicts(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    project_id: &str,
    dir: &str,
    names: Vec<String>,
) -> Result<Vec<String>, String> {
    let v: serde_json::Value = peer_post(
        host,
        lan,
        device_id,
        "/api/fs/exists",
        serde_json::json!({ "project_id": project_id, "dir": dir, "names": names }),
    )
    .await?;
    Ok(serde_json::from_value(v.get("existing").cloned().unwrap_or_default()).unwrap_or_default())
}

/// Which top-level names a pull of `items` would collide with in local `dest_dir`.
pub fn local_conflicts(dest_dir: &str, items: &[consent::RequestedFile]) -> Vec<String> {
    let names: Vec<String> = items.iter().map(|i| super::files::top_name(&i.project_name, &i.path)).collect();
    super::files::conflicts(std::path::Path::new(dest_dir), &names)
}

/// Options for [`pull_files`] / [`push_files`].
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferOpts {
    #[serde(default)]
    pub policy: super::files::ConflictPolicy,
    #[serde(default)]
    pub renames: std::collections::HashMap<String, String>,
}

/// Why one step of a resumable transfer stopped.
enum Fail {
    /// Connection dropped / timed out / network changed — pause and retry.
    Net(String),
    /// The approval ticket expired (long pause) — ask again.
    Expired,
    /// The user paused or cancelled.
    Interrupted,
    /// A hard refusal or local error — give up.
    Fatal(String),
}

/// Classify a peer response for the resumable loops.
async fn classify(lan: &LanState, peer: &Peer, resp: Result<reqwest::Response, reqwest::Error>) -> Result<reqwest::Response, Fail> {
    let r = resp.map_err(|e| Fail::Net(e.to_string()))?;
    let status = r.status();
    if matches!(status.as_u16(), 502..=504) {
        return Err(Fail::Net(format!("HTTP {status}")));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        let body: serde_json::Value = r.json().await.unwrap_or_default();
        let msg = body.get("error").and_then(|v| v.as_str()).unwrap_or("refused").to_string();
        return Err(if msg.contains("expired") || msg.contains("needs approval") { Fail::Expired } else { Fail::Fatal(msg) });
    }
    check_response(lan, peer, r).await.map_err(Fail::Fatal)
}

/// Paused for a lost connection: wait until the device answers again (or
/// the user cancels). Honors a user pause on top.
async fn wait_for_peer(host: &HostRef, lan: &LanState, device_id: &str, handle: &crate::transfers::Handle, why: &str) -> Result<(), String> {
    let name = paired(host, device_id).map(|p| p.display_name()).unwrap_or_else(|_| "the other machine".into());
    tracing::info!(device = %device_id, transfer = %handle.id, why, "peer: transfer paused — connection lost");
    handle.net_pause(&format!("Connection lost — waiting for {name}"));
    loop {
        if handle.is_cancelled() {
            return Err("Cancelled".into());
        }
        if !handle.is_user_paused() {
            lan.lock().working_addr.remove(device_id);
            if let Ok(peer) = paired(host, device_id) {
                if resolve_addr(host, lan, &peer).await.is_ok() {
                    tracing::info!(device = %device_id, transfer = %handle.id, "peer: connection back — resuming transfer");
                    handle.net_resume();
                    return Ok(());
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
            _ = handle.changed() => {}
        }
    }
}

/// Before each step: wait out a user pause; error on cancel.
async fn checkpoint(handle: &crate::transfers::Handle) -> Result<(), String> {
    if handle.is_user_paused() {
        handle.wait_user_resume().await;
    }
    if handle.is_cancelled() {
        return Err("Cancelled".into());
    }
    Ok(())
}

/// A read that delivers nothing for this long counts as a dropped connection.
const STALL: std::time::Duration = std::time::Duration::from_secs(30);

/// Approval for `request`, cancellable from the tray.
async fn approval_for(host: &HostRef, lan: &LanState, device_id: &str, request: &consent::TransferRequest, handle: &crate::transfers::Handle) -> Result<String, String> {
    let name = paired(host, device_id).map(|p| p.display_name()).unwrap_or_default();
    handle.stage("waiting", "approval", &format!("Waiting for {name} to approve…"));
    let t = tokio::select! {
        t = request_approval(host, lan, device_id, request) => t,
        _ = handle.cancelled() => {
            let _ = cancel_outgoing(lan, device_id);
            Err("Cancelled".to_string())
        }
    }?;
    handle.stage("running", if request.kind == "push" { "uploading" } else { "downloading" }, "");
    Ok(t)
}

/// Download one manifest file into its part file from the current offset.
#[allow(clippy::too_many_arguments)]
async fn fetch_file(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    peer: &Peer,
    ticket: &str,
    entry: &super::files::ManifestEntry,
    target: &std::path::Path,
    handle: &crate::transfers::Handle,
    base_done: u64,
) -> Result<(), Fail> {
    let offset = super::files::received_len(target, entry.size);
    if offset < entry.size || entry.size == 0 {
        let gen = crate::transfers::network_generation();
        let (client, base, token) = connect_with(host, lan, device_id, &[(super::TICKET_HEADER, ticket.to_string())])
            .await
            .map_err(Fail::Net)?;
        let resp = client
            .post(format!("{base}/api/fs/read"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "project_id": entry.project_id, "path": entry.src, "offset": offset }))
            .send()
            .await;
        let resp = classify(lan, peer, resp).await?;
        let mut file = super::files::open_part_at(target, offset).map_err(Fail::Fatal)?;
        let mut got = offset;
        use futures_util::StreamExt;
        use std::io::Write;
        let mut stream = resp.bytes_stream();
        loop {
            tokio::select! {
                next = tokio::time::timeout(STALL, stream.next()) => match next {
                    Err(_) => return Err(Fail::Net("no data for 30s".into())),
                    Ok(None) => break,
                    Ok(Some(Err(e))) => return Err(Fail::Net(e.to_string())),
                    Ok(Some(Ok(chunk))) => {
                        file.write_all(&chunk).map_err(|e| Fail::Fatal(e.to_string()))?;
                        got += chunk.len() as u64;
                        handle.progress("downloading", base_done + got, 0);
                    }
                },
                _ = handle.interrupted() => return Err(Fail::Interrupted),
                _ = crate::transfers::network_changed_since(gen) => return Err(Fail::Net("network changed".into())),
            }
        }
        file.flush().map_err(|e| Fail::Fatal(e.to_string()))?;
        if got < entry.size {
            return Err(Fail::Net(format!("stream ended at {got} of {} bytes", entry.size)));
        }
    }
    super::files::finish_part(target).map_err(Fail::Fatal)
}

/// Pull files / folders from a paired device into local folder `dest_dir`.
/// Lists every file first (size + ETA), asks for one batch approval, plans
/// name clashes once, then fetches file by file from each file's received
/// offset. A lost connection or network change pauses it and it resumes on
/// its own when the device is reachable again; the user can pause / resume /
/// cancel from the tray. A pause long enough for the approval to expire asks
/// again.
pub async fn pull_files(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    items: Vec<consent::RequestedFile>,
    dest_dir: String,
    opts: FileTransferOpts,
) -> Result<super::files::UnpackSummary, String> {
    if items.is_empty() {
        return Err("Nothing selected".into());
    }
    let peer = paired(host, device_id)?;
    let label = label_for_items(&items);
    let manifest: serde_json::Value = peer_post(host, lan, device_id, "/api/fs/manifest", serde_json::json!({ "items": items })).await?;
    let entries: Vec<super::files::ManifestEntry> =
        serde_json::from_value(manifest.get("entries").cloned().unwrap_or_default()).map_err(|e| e.to_string())?;
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let files = entries.iter().filter(|e| !e.is_dir).count() as u64;
    let handle = crate::transfers::begin(host.emitter(), "pull", &label, &peer.display_name(), total, files);
    handle.set_can_pause();
    let result = async {
        let request = consent::TransferRequest { kind: "pull".into(), files: items.clone(), total_bytes: Some(total), ..Default::default() };
        let mut ticket = approval_for(host, lan, device_id, &request, &handle).await?;
        let dest = std::path::PathBuf::from(&dest_dir);
        let mut tops: Vec<String> = Vec::new();
        for e in &entries {
            let top = e.rel.split('/').next().unwrap_or_default().to_string();
            if !tops.contains(&top) {
                tops.push(top);
            }
        }
        let (plan, mut summary) = super::files::plan_tops(&dest, &tops, opts.policy, &opts.renames)?;
        let mut done: u64 = 0;
        for entry in &entries {
            let Some(target) = super::files::planned_target(&dest, &plan, &entry.rel)? else {
                done += entry.size;
                continue;
            };
            if entry.is_dir {
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                continue;
            }
            loop {
                checkpoint(&handle).await?;
                match fetch_file(host, lan, device_id, &peer, &ticket, entry, &target, &handle, done).await {
                    Ok(()) => break,
                    Err(Fail::Interrupted) => continue,
                    Err(Fail::Net(why)) => wait_for_peer(host, lan, device_id, &handle, &why).await?,
                    Err(Fail::Expired) => ticket = approval_for(host, lan, device_id, &request, &handle).await?,
                    Err(Fail::Fatal(e)) => return Err(format!("{}: {e}", entry.rel)),
                }
            }
            done += entry.size;
            summary.files += 1;
            summary.bytes += entry.size;
            handle.progress("downloading", done, 0);
        }
        Ok(summary)
    }
    .await;
    handle.finish(result.as_ref().map(|_| Some(dest_dir.clone())).map_err(|e| e.clone()));
    result
}

/// Upload local files / folders (absolute paths) to folder `dir` of a paired
/// device's project after one batch approval there. Resumable like
/// [`pull_files`]: the receiver keeps partial files, and each file continues
/// from the offset it reports.
pub async fn push_files(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    local_paths: Vec<String>,
    project_id: String,
    dir: String,
    opts: FileTransferOpts,
) -> Result<serde_json::Value, String> {
    if local_paths.is_empty() {
        return Err("Nothing selected".into());
    }
    super::files::safe_rel(&dir)?;
    let peer = paired(host, device_id)?;
    let specs: Vec<(String, String, std::path::PathBuf, String)> = local_paths
        .iter()
        .filter_map(|p| {
            let abs = std::path::PathBuf::from(p);
            let name = abs.file_name()?.to_string_lossy().into_owned();
            Some((String::new(), abs.to_string_lossy().replace('\\', "/"), abs, name))
        })
        .collect();
    let names: Vec<String> = specs.iter().map(|s| s.3.clone()).collect();
    let entries = tokio::task::spawn_blocking(move || super::files::manifest(&specs)).await.map_err(|e| e.to_string())?;
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let files = entries.iter().filter(|e| !e.is_dir).count() as u64;
    let label = match names.as_slice() {
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} + {} more", rest.len()),
        [] => "files".into(),
    };
    let handle = crate::transfers::begin(host.emitter(), "push", &label, &peer.display_name(), total, files);
    handle.set_can_pause();
    let result = async {
        let request = consent::TransferRequest {
            kind: "push".into(),
            files: vec![consent::RequestedFile { project_id: project_id.clone(), project_name: String::new(), path: dir.clone(), is_dir: true, names: names.clone() }],
            total_bytes: Some(total),
            ..Default::default()
        };
        let mut ticket = approval_for(host, lan, device_id, &request, &handle).await?;
        let wire: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| serde_json::json!({ "rel": e.rel, "is_dir": e.is_dir, "size": e.size }))
            .collect();
        let begin_body = serde_json::json!({
            "project_id": project_id, "dir": dir, "policy": opts.policy, "renames": opts.renames,
            "entries": wire, "label": label,
        });
        let session_id = loop {
            checkpoint(&handle).await?;
            let attempt = async {
                let (c, b, t) = connect_with(host, lan, device_id, &[(super::TICKET_HEADER, ticket.clone())]).await.map_err(Fail::Net)?;
                let r = classify(lan, &peer, c.post(format!("{b}/api/fs/upload/begin")).bearer_auth(&t).json(&begin_body).send().await).await?;
                let v: serde_json::Value = r.json().await.map_err(|e| Fail::Net(e.to_string()))?;
                v.get("id").and_then(|x| x.as_str()).map(str::to_string).ok_or_else(|| Fail::Fatal("no upload id".into()))
            }
            .await;
            match attempt {
                Ok(id) => break id,
                Err(Fail::Net(why)) => wait_for_peer(host, lan, device_id, &handle, &why).await?,
                Err(Fail::Expired) => ticket = approval_for(host, lan, device_id, &request, &handle).await?,
                Err(Fail::Interrupted) => continue,
                Err(Fail::Fatal(e)) => return Err(e),
            }
        };
        let mut done: u64 = 0;
        for entry in entries.iter().filter(|e| !e.is_dir) {
            loop {
                checkpoint(&handle).await?;
                match send_file(host, lan, device_id, &peer, &session_id, entry, &handle, done).await {
                    Ok(()) => break,
                    Err(Fail::Interrupted) => continue,
                    Err(Fail::Net(why)) => wait_for_peer(host, lan, device_id, &handle, &why).await?,
                    Err(Fail::Expired) => return Err("the upload approval expired — send it again".into()),
                    Err(Fail::Fatal(e)) => return Err(format!("{}: {e}", entry.rel)),
                }
            }
            done += entry.size;
            handle.progress("uploading", done, 0);
        }
        loop {
            checkpoint(&handle).await?;
            let attempt = async {
                let (c, b, t) = connect_with(host, lan, device_id, &[]).await.map_err(Fail::Net)?;
                let r = classify(lan, &peer, c.post(format!("{b}/api/fs/upload/finish")).bearer_auth(&t).json(&serde_json::json!({ "id": session_id })).send().await).await?;
                r.json::<serde_json::Value>().await.map_err(|e| Fail::Net(e.to_string()))
            }
            .await;
            match attempt {
                Ok(v) => return Ok(v),
                Err(Fail::Net(why)) => wait_for_peer(host, lan, device_id, &handle, &why).await?,
                Err(Fail::Interrupted) => continue,
                Err(Fail::Expired) | Err(Fail::Fatal(_)) => return Err("finishing the upload failed".into()),
            }
        }
    }
    .await;
    handle.finish(result.as_ref().map(|_| None).map_err(|e| e.clone()));
    result
}

/// Upload one local file from the offset the receiver reports.
#[allow(clippy::too_many_arguments)]
async fn send_file(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    peer: &Peer,
    session_id: &str,
    entry: &super::files::ManifestEntry,
    handle: &crate::transfers::Handle,
    base_done: u64,
) -> Result<(), Fail> {
    let gen = crate::transfers::network_generation();
    let (client, base, token) = connect_with(host, lan, device_id, &[]).await.map_err(Fail::Net)?;
    let r = classify(lan, peer, client
        .post(format!("{base}/api/fs/upload/offset"))
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(20))
        .json(&serde_json::json!({ "id": session_id, "rel": entry.rel }))
        .send()
        .await).await?;
    let v: serde_json::Value = r.json().await.map_err(|e| Fail::Net(e.to_string()))?;
    if v.get("skip").and_then(|x| x.as_bool()) == Some(true) {
        return Ok(());
    }
    let offset = v.get("offset").and_then(|x| x.as_u64()).unwrap_or(0);
    if offset >= entry.size && entry.size > 0 {
        return Ok(());
    }
    let mut file = tokio::fs::File::open(&entry.src).await.map_err(|e| Fail::Fatal(format!("{}: {e}", entry.src)))?;
    {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| Fail::Fatal(e.to_string()))?;
    }
    let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(offset));
    let progress = std::sync::Arc::clone(&sent);
    let stream = {
        use futures_util::StreamExt;
        tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024).map(move |chunk| {
            if let Ok(c) = &chunk {
                progress.fetch_add(c.len() as u64, std::sync::atomic::Ordering::SeqCst);
            }
            chunk
        })
    };
    let meta = serde_json::json!({ "id": session_id, "rel": entry.rel, "offset": offset }).to_string();
    let send = client
        .post(format!("{base}/api/fs/upload/chunk"))
        .bearer_auth(&token)
        .header(super::listener::CHUNK_HEADER, meta)
        .body(reqwest::Body::wrap_stream(stream))
        .send();
    tokio::pin!(send);
    let mut last = offset;
    let mut last_move = std::time::Instant::now();
    let resp = loop {
        tokio::select! {
            r = &mut send => break r,
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                let now = sent.load(std::sync::atomic::Ordering::SeqCst);
                handle.progress("uploading", base_done + now, 0);
                if now != last {
                    last = now;
                    last_move = std::time::Instant::now();
                } else if last_move.elapsed() > STALL {
                    return Err(Fail::Net("upload stalled for 30s".into()));
                }
            }
            _ = handle.interrupted() => return Err(Fail::Interrupted),
            _ = crate::transfers::network_changed_since(gen) => return Err(Fail::Net("network changed".into())),
        }
    };
    let r = classify(lan, peer, resp).await?;
    let v: serde_json::Value = r.json().await.map_err(|e| Fail::Net(e.to_string()))?;
    let received = v.get("received").and_then(|x| x.as_u64()).unwrap_or(0);
    if received < entry.size && v.get("skipped").is_none() {
        return Err(Fail::Net(format!("receiver has {received} of {} bytes", entry.size)));
    }
    Ok(())
}

/// Short label for a selection.
fn label_for_items(items: &[consent::RequestedFile]) -> String {
    match items {
        [] => "files".into(),
        [one] => super::files::top_name(&one.project_name, &one.path),
        [first, rest @ ..] => format!("{} + {} more", super::files::top_name(&first.project_name, &first.path), rest.len()),
    }
}

/// Ask a paired device for one-time permission to browse all its metadata.
pub async fn request_meta_access(host: &HostRef, lan: &LanState, device_id: &str) -> Result<(), String> {
    let request = consent::TransferRequest { kind: "meta_access".into(), meta_access: true, ..Default::default() };
    request_approval(host, lan, device_id, &request).await.map(|_| ())
}

/// A paired device's metadata for browsing: `granted = false` means only
/// what it shares, as hashes (ask with [`request_meta_access`]).
pub async fn meta_browse(host: &HostRef, lan: &LanState, device_id: &str) -> Result<serde_json::Value, String> {
    let peer = paired(host, device_id)?;
    let (client, base, token) = connect_with(host, lan, device_id, &[("x-rustic-meta-view", "1".into())]).await?;
    let resp = client
        .get(format!("{base}/api/sync/meta"))
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Could not reach {}: {e}", peer.display_name()))?;
    let resp = check_response(lan, &peer, resp).await?;
    let granted = resp
        .headers()
        .get("x-rustic-meta-granted")
        .and_then(|v| v.to_str().ok())
        == Some("1");
    let bundle: crate::meta_sync::MetaBundle = resp.json().await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "granted": granted, "items": bundle.items }))
}

/// Revoke (or set) a paired device's metadata-browsing grant on this machine.
pub fn set_meta_view(host: &HostRef, device_id: &str, allowed: bool) -> Result<(), String> {
    let dir = host.data_dir()?;
    let mut peers = super::load_peers(&dir);
    let p = peers.iter_mut().find(|p| p.device_id == device_id).ok_or("That device is not paired")?;
    p.meta_view = allowed;
    super::save_peers(&dir, &peers)
}
