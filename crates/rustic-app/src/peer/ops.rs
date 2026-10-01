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

/// Start the TLS listener on port 47820 (+ fallback) and mDNS (idempotent).
pub async fn start(host: &HostRef, lan: &LanState) -> Result<(), String> {
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
    Ok(())
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

/// Ask a paired device's `/lan/info`; on success mark it online and remember
/// the address that worked.
async fn probe_peer(host: HostRef, lan: LanState, peer: Peer) {
    for addr in candidate_addrs(&lan, &peer) {
        let Ok(client) = super::peer_client(&peer.fingerprint, &addr, &[]) else {
            continue;
        };
        let resp = client
            .get(format!("{}/lan/info", super::base_url(&addr)))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
        let Ok(resp) = resp else { continue };
        let Ok(body) = resp.json::<serde_json::Value>().await else {
            continue;
        };
        if body.get("device_id").and_then(|v| v.as_str()) != Some(peer.device_id.as_str()) {
            continue;
        }
        lan.lock().mark_seen(&peer.device_id, Some(addr.clone()));
        if peer.addr.as_deref() != Some(addr.as_str()) {
            if let Ok(dir) = host.data_dir() {
                let mut peers = super::load_peers(&dir);
                if let Some(p) = peers.iter_mut().find(|p| p.device_id == peer.device_id) {
                    p.addr = Some(addr);
                    let _ = super::save_peers(&dir, &peers);
                }
            }
        }
        return;
    }
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
        let (seen_recently, seen_addr) = {
            let inner = lan.lock();
            (
                inner.recently_seen(&p.device_id),
                inner.seen.get(&p.device_id).and_then(|s| s.addr.clone()),
            )
        };
        let online = running && (found.is_some() || seen_recently);
        if running && !online {
            let due = {
                let mut inner = lan.lock();
                let due = inner
                    .probed
                    .get(&p.device_id)
                    .is_none_or(|t| t.elapsed() >= PROBE_EVERY);
                if due {
                    inner
                        .probed
                        .insert(p.device_id.clone(), std::time::Instant::now());
                }
                due
            };
            if due {
                tokio::spawn(probe_peer(host.clone(), lan.clone(), p.clone()));
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
    let d = inner
        .candidate(device_id)
        .ok_or("That device is not on the network right now")?;
    Ok(super::pairing_code(&me.fingerprint, &d.fingerprint))
}

/// Ask `device_id` to pair. Waits for the other machine's Accept/Decline.
/// Returns the other device's name.
pub async fn pair(host: &HostRef, lan: &LanState, device_id: &str) -> Result<String, String> {
    let (me, target) = {
        let inner = lan.lock();
        let me = inner
            .identity
            .clone()
            .ok_or("Turn on local-network sync first")?;
        let d = inner
            .candidate(device_id)
            .ok_or("That device is not on the network right now")?;
        (me, d)
    };
    let client = super::peer_client(&target.fingerprint, &target.addr, &[])?;
    let token_in = super::random_token()?;
    let resp = client
        .post(format!("{}/lan/pair", super::base_url(&target.addr)))
        .timeout(std::time::Duration::from_secs(100))
        .json(&serde_json::json!({
            "device_id": me.device_id,
            "name": me.device_name,
            "fingerprint": me.fingerprint,
            "token": token_in,
        }))
        .send()
        .await
        .map_err(|e| format!("Could not reach {}: {e}", target.name))?;
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
        },
    )?;
    {
        let mut inner = lan.lock();
        inner.manual.remove(&target.device_id);
        inner.mark_seen(&target.device_id, Some(target.addr.clone()));
    }
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

/// Forget a paired device (it must pair again to sync).
pub fn forget(host: &HostRef, device_id: &str) -> Result<(), String> {
    let dir = host.data_dir()?;
    let mut peers = super::load_peers(&dir);
    peers.retain(|p| p.device_id != device_id);
    super::save_peers(&dir, &peers)
}

/// Client + peer API base (`<peer>/lan`) + token for a paired device,
/// sending `extra` headers.
pub fn connect_with(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    extra: &[(&'static str, String)],
) -> Result<(reqwest::Client, String, String), String> {
    let peer = super::load_peers(&host.data_dir()?)
        .into_iter()
        .find(|p| p.device_id == device_id)
        .ok_or("That device is not paired — pair it first")?;
    let addr = candidate_addrs(lan, &peer)
        .into_iter()
        .next()
        .ok_or("That device is not on the network right now")?;
    let client = super::peer_client(&peer.fingerprint, &addr, extra)?;
    Ok((client, super::api_base(&addr), peer.token_out))
}

/// Client + peer API base + token for a paired device.
pub fn connect(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
) -> Result<(reqwest::Client, String, String), String> {
    connect_with(host, lan, device_id, &[])
}

/// Push to a paired device: everything, or one project.
pub async fn push(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    project_id: Option<String>,
) -> Result<String, String> {
    let (client, base, token) = connect(host, lan, device_id)?;
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
    let (client, base, token) = connect(host, lan, device_id)?;
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
    let (client, base, token) = connect(host, lan, device_id)?;
    cs::list_projects_env(&client, &base, &token).await
}

/// Metadata diff against a paired device.
pub async fn meta_preview(
    host: &HostRef,
    lan: &LanState,
    device_id: &str,
    direction: &str,
) -> Result<Vec<crate::meta_sync::MetaDiffEntry>, String> {
    let (client, base, token) = connect(host, lan, device_id)?;
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
    let (client, base, token) = connect(host, lan, device_id)?;
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
    let (client, base, token) = connect(host, lan, device_id)?;
    let request = consent::TransferRequest {
        kind: direction.to_string(),
        projects: projects
            .iter()
            .map(|p| consent::RequestedProject { id: p.id.clone(), name: p.name.clone(), exists: false })
            .collect(),
        meta: meta.clone(),
    };
    let resp = client
        .post(format!("{base}/request"))
        .bearer_auth(&token)
        .timeout(consent::PROMPT_TIMEOUT + std::time::Duration::from_secs(10))
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Could not reach {peer_name}: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(body
            .get("error")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{peer_name} refused the request (HTTP {status})")));
    }
    let Some(ticket) = body
        .get("approved")
        .and_then(|v| v.as_bool())
        .filter(|a| *a)
        .and_then(|_| body.get("ticket").and_then(|v| v.as_str()))
        .map(str::to_string)
    else {
        return Err(format!("{peer_name} declined (or didn't answer in time)"));
    };

    let rep = cs::reporter(host, direction);
    let mut out = SyncItemsResult { projects_ok: 0, failures: Vec::new(), meta: None };
    for p in projects {
        let label = if p.name.is_empty() { p.id.clone() } else { p.name.clone() };
        let res = async {
            let (c, b, t) = connect_with(
                host,
                lan,
                device_id,
                &[(super::TICKET_HEADER, ticket.clone()), (super::PROJECT_HEADER, p.id.clone())],
            )?;
            if direction == "push" {
                cs::push_project_env(host, &c, &b, &t, p.id.clone(), &rep, cs::LAN_STREAMS).await
            } else {
                cs::pull_project_env(host, &c, &b, &t, p.id.clone(), p.target_parent.clone(), &rep, cs::LAN_STREAMS).await
            }
        }
        .await;
        match res {
            Ok(_) => out.projects_ok += 1,
            Err(e) => out.failures.push(format!("{label}: {e}")),
        }
    }

    if !meta.is_empty() {
        let keys: std::collections::HashSet<String> = meta.iter().map(|m| m.key.clone()).collect();
        let (c, b, t) = connect_with(host, lan, device_id, &[(super::TICKET_HEADER, ticket.clone())])?;
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

/// Startup: restore the listener (and Cloudflare tunnel) if they were on.
pub async fn restore(host: &HostRef, lan: &LanState) {
    let Ok(dir) = host.data_dir() else { return };
    if !super::is_enabled_persisted(&dir) {
        return;
    }
    if let Err(e) = start(host, lan).await {
        tracing::warn!("LAN sync did not start: {e}");
        return;
    }
    if super::internet_mode(&dir).as_deref() == Some("cloudflare") {
        if let Err(e) = start_tunnel(host, lan).await {
            tracing::warn!("Cloudflare tunnel did not start: {e}");
        }
    }
}
