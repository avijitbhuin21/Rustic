//! Registry of running / finished transfers (peer file + project syncs) for
//! the top-bar transfers tray: size up front, live progress, speed, ETA,
//! cancel, "open location" and "clear". Every change is emitted as
//! `rustic:transfer` with the full [`TransferInfo`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

use crate::EventEmitter;

/// Snapshot of one transfer, as shown in the tray.
#[derive(Debug, Clone, Serialize)]
pub struct TransferInfo {
    pub id: String,
    /// `"pull"` (incoming to this machine) or `"push"` (outgoing).
    pub direction: String,
    pub label: String,
    /// Other machine's display name.
    pub peer: String,
    /// `"waiting"` (for approval), `"running"`, `"done"`, `"failed"`, `"cancelled"`.
    pub state: String,
    pub phase: String,
    pub detail: String,
    pub done: u64,
    /// 0 = unknown.
    pub total: u64,
    pub files: u64,
    pub speed_bps: f64,
    pub eta_secs: Option<u64>,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    /// Local folder the result landed in (pulls) — for "open location".
    pub location: Option<String>,
    pub error: Option<String>,
    /// Whether Pause / Resume are offered (resumable file transfers).
    pub can_pause: bool,
    /// Paused by the user or by a lost connection.
    pub paused: bool,
    /// Why it's paused ("Paused", "Connection lost — waiting for X").
    pub paused_reason: Option<String>,
}

/// Live handle to one transfer.
pub struct Handle {
    pub id: String,
    cancel: AtomicBool,
    /// Paused by the user (Pause button).
    user_paused: AtomicBool,
    /// Paused because the connection dropped (auto-resumes on reconnect).
    net_paused: AtomicBool,
    notify: tokio::sync::Notify,
    emitter: Arc<dyn EventEmitter>,
    speed: Mutex<SpeedState>,
}

/// Bumped whenever this machine's network changes (IP switch); running
/// transfers treat it as a dropped connection and pause until the peer is
/// reachable again.
static NET_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static NET_NOTIFY: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Signal every running transfer that the network changed.
pub fn network_changed() {
    NET_GEN.fetch_add(1, Ordering::SeqCst);
    NET_NOTIFY.notify_waiters();
}

/// Current network generation (compare with a later value to detect changes).
pub fn network_generation() -> u64 {
    NET_GEN.load(Ordering::SeqCst)
}

/// Resolves when the network generation moves past `since`.
pub async fn network_changed_since(since: u64) {
    loop {
        let n = NET_NOTIFY.notified();
        if NET_GEN.load(Ordering::SeqCst) != since {
            return;
        }
        n.await;
    }
}

struct SpeedState {
    last_at: Instant,
    last_done: u64,
    bps: f64,
    last_emit: Instant,
}

#[derive(Default)]
struct Registry {
    order: Vec<String>,
    infos: HashMap<String, TransferInfo>,
    handles: HashMap<String, Arc<Handle>>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// Run `f` on the registry.
fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut g = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Registry::default))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Start tracking a transfer. `total` = bytes when known up front (0 = unknown).
pub fn begin(
    emitter: Arc<dyn EventEmitter>,
    direction: &str,
    label: &str,
    peer: &str,
    total: u64,
    files: u64,
) -> Arc<Handle> {
    let id = crate::peer::random_token().map(|t| t[..16].to_string()).unwrap_or_else(|_| now_ms().to_string());
    let handle = Arc::new(Handle {
        id: id.clone(),
        cancel: AtomicBool::new(false),
        user_paused: AtomicBool::new(false),
        net_paused: AtomicBool::new(false),
        notify: tokio::sync::Notify::new(),
        emitter,
        speed: Mutex::new(SpeedState {
            last_at: Instant::now(),
            last_done: 0,
            bps: 0.0,
            last_emit: Instant::now() - std::time::Duration::from_secs(1),
        }),
    });
    let info = TransferInfo {
        id: id.clone(),
        direction: direction.to_string(),
        label: label.to_string(),
        peer: peer.to_string(),
        state: "running".into(),
        phase: "starting".into(),
        detail: String::new(),
        done: 0,
        total,
        files,
        speed_bps: 0.0,
        eta_secs: None,
        started_ms: now_ms(),
        finished_ms: None,
        location: None,
        error: None,
        can_pause: false,
        paused: false,
        paused_reason: None,
    };
    with(|r| {
        r.order.push(id.clone());
        r.infos.insert(id.clone(), info.clone());
        r.handles.insert(id.clone(), Arc::clone(&handle));
        // Keep the tray bounded: drop the oldest finished entries.
        while r.order.len() > 50 {
            let Some(pos) = r.order.iter().position(|i| r.infos.get(i).is_some_and(|x| x.finished_ms.is_some())) else { break };
            let old = r.order.remove(pos);
            r.infos.remove(&old);
            r.handles.remove(&old);
        }
    });
    handle.emit_info(&info);
    handle
}

impl Handle {
    fn emit_info(&self, info: &TransferInfo) {
        self.emitter
            .emit_json("rustic:transfer", serde_json::to_value(info).unwrap_or_default());
    }

    /// Apply `f` to this transfer's info and emit it (throttled unless `force`).
    fn update_with(&self, force: bool, f: impl FnOnce(&mut TransferInfo)) {
        let info = with(|r| {
            r.infos.get_mut(&self.id).map(|i| {
                f(i);
                i.clone()
            })
        });
        let Some(info) = info else { return };
        if !force {
            let mut s = self.speed.lock().unwrap_or_else(|p| p.into_inner());
            if s.last_emit.elapsed() < std::time::Duration::from_millis(250) {
                return;
            }
            s.last_emit = Instant::now();
        }
        self.emit_info(&info);
    }

    /// Bytes moved so far (and the total, when it became known).
    pub fn progress(&self, phase: &str, done: u64, total: u64) {
        let bps = {
            let mut s = self.speed.lock().unwrap_or_else(|p| p.into_inner());
            let dt = s.last_at.elapsed().as_secs_f64();
            if dt >= 0.5 {
                let inst = done.saturating_sub(s.last_done) as f64 / dt;
                s.bps = if s.bps == 0.0 { inst } else { s.bps * 0.7 + inst * 0.3 };
                s.last_at = Instant::now();
                s.last_done = done;
            }
            s.bps
        };
        self.update_with(false, |i| {
            i.state = "running".into();
            i.phase = phase.to_string();
            i.done = done;
            if total > 0 {
                i.total = total;
            }
            i.speed_bps = bps;
            i.eta_secs = (i.total > done && bps > 1.0).then(|| ((i.total - done) as f64 / bps).ceil() as u64);
        });
    }

    /// Phase change without byte movement (always emitted).
    pub fn stage(&self, state: &str, phase: &str, detail: &str) {
        self.update_with(true, |i| {
            i.state = state.to_string();
            i.phase = phase.to_string();
            i.detail = detail.to_string();
        });
    }

    /// Set the total once it's known.
    pub fn set_total(&self, total: u64, files: u64) {
        self.update_with(true, |i| {
            i.total = total;
            i.files = files;
        });
    }

    /// Mark finished: `Ok(location)` or `Err(message)`.
    pub fn finish(&self, result: Result<Option<String>, String>) {
        let cancelled = self.is_cancelled();
        self.update_with(true, |i| {
            i.finished_ms = Some(now_ms());
            i.eta_secs = None;
            i.speed_bps = 0.0;
            match result {
                Ok(loc) => {
                    i.state = "done".into();
                    i.phase = "done".into();
                    if i.total > 0 {
                        i.done = i.total;
                    }
                    i.location = loc;
                }
                Err(_) if cancelled => {
                    i.state = "cancelled".into();
                    i.phase = "cancelled".into();
                }
                Err(e) => {
                    i.state = "failed".into();
                    i.phase = "failed".into();
                    i.error = Some(e);
                }
            }
        });
        with(|r| r.handles.remove(&self.id));
    }

    /// Whether the user cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Resolves once the user cancels.
    pub async fn cancelled(&self) {
        loop {
            let n = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }

    /// Request cancellation.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Offer Pause / Resume in the tray for this transfer.
    pub fn set_can_pause(&self) {
        self.update_with(true, |i| i.can_pause = true);
    }

    /// Whether the user or a lost connection has paused it.
    pub fn is_paused(&self) -> bool {
        self.user_paused.load(Ordering::SeqCst) || self.net_paused.load(Ordering::SeqCst)
    }

    /// Whether the user paused it (as opposed to a lost connection).
    pub fn is_user_paused(&self) -> bool {
        self.user_paused.load(Ordering::SeqCst)
    }

    /// Pause (user).
    pub fn pause(&self) {
        self.user_paused.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        self.refresh_pause_state();
    }

    /// Resume (user). A connection pause still holds until the peer is back.
    pub fn resume(&self) {
        self.user_paused.store(false, Ordering::SeqCst);
        self.notify.notify_waiters();
        self.refresh_pause_state();
    }

    /// Mark paused because the connection dropped (`reason` shown in the tray).
    pub fn net_pause(&self, reason: &str) {
        self.net_paused.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        let reason = reason.to_string();
        self.update_with(true, |i| {
            i.paused = true;
            i.speed_bps = 0.0;
            i.eta_secs = None;
            if i.paused_reason.as_deref().is_none_or(|r| r != "Paused") {
                i.paused_reason = Some(reason);
            }
        });
    }

    /// The connection is back.
    pub fn net_resume(&self) {
        self.net_paused.store(false, Ordering::SeqCst);
        self.notify.notify_waiters();
        self.refresh_pause_state();
    }

    /// Re-derive the tray's paused flag + reason from both pause sources.
    fn refresh_pause_state(&self) {
        let user = self.user_paused.load(Ordering::SeqCst);
        let net = self.net_paused.load(Ordering::SeqCst);
        self.update_with(true, |i| {
            i.paused = user || net;
            if user {
                i.paused_reason = Some("Paused".into());
                i.speed_bps = 0.0;
                i.eta_secs = None;
            } else if !net {
                i.paused_reason = None;
            }
        });
    }

    /// Resolves when anything changes (pause, resume, cancel).
    pub async fn changed(&self) {
        self.notify.notified().await;
    }

    /// Resolves once the user pauses or cancels.
    pub async fn interrupted(&self) {
        loop {
            let n = self.notify.notified();
            if self.is_cancelled() || self.is_user_paused() {
                return;
            }
            n.await;
        }
    }

    /// Wait while the user has it paused (returns early on cancel).
    pub async fn wait_user_resume(&self) {
        loop {
            let n = self.notify.notified();
            if self.is_cancelled() || !self.is_user_paused() {
                return;
            }
            n.await;
        }
    }
}

/// Pause a running transfer.
pub fn pause(id: &str) -> Result<(), String> {
    let h = with(|r| r.handles.get(id).cloned()).ok_or("That transfer already finished")?;
    h.pause();
    Ok(())
}

/// Resume a paused transfer.
pub fn resume(id: &str) -> Result<(), String> {
    let h = with(|r| r.handles.get(id).cloned()).ok_or("That transfer already finished")?;
    h.resume();
    Ok(())
}

/// Resume every connection-paused transfer to `peer` (it came back).
pub fn peer_reachable(peer: &str) {
    let hs: Vec<Arc<Handle>> = with(|r| {
        r.handles
            .iter()
            .filter(|(id, _)| r.infos.get(*id).is_some_and(|i| i.peer == peer))
            .map(|(_, h)| Arc::clone(h))
            .collect()
    });
    for h in hs {
        h.notify.notify_waiters();
    }
}

impl Handle {
}

/// All transfers, oldest first.
pub fn list() -> Vec<TransferInfo> {
    with(|r| r.order.iter().filter_map(|i| r.infos.get(i).cloned()).collect())
}

/// Cancel a running transfer.
pub fn cancel(id: &str) -> Result<(), String> {
    let h = with(|r| r.handles.get(id).cloned()).ok_or("That transfer already finished")?;
    h.cancel();
    h.stage("running", "cancelling", "");
    Ok(())
}

/// Remove a finished transfer from the list (`None` = all finished ones).
pub fn clear(id: Option<&str>) {
    with(|r| {
        let finished = |i: &String| r.infos.get(i).is_some_and(|x| x.finished_ms.is_some());
        let drop: Vec<String> = r
            .order
            .iter()
            .filter(|i| id.is_none_or(|want| want == i.as_str()) && finished(i))
            .cloned()
            .collect();
        for d in drop {
            r.order.retain(|i| i != &d);
            r.infos.remove(&d);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Null;
    impl EventEmitter for Null {
        fn emit_json(&self, _event: &str, _payload: serde_json::Value) {}
    }

    #[test]
    fn lifecycle_progress_cancel_clear() {
        let h = begin(Arc::new(Null), "pull", "src", "Laptop", 1000, 3);
        h.progress("downloading", 400, 0);
        let i = list().into_iter().find(|i| i.id == h.id).unwrap();
        assert_eq!((i.done, i.total, i.state.as_str()), (400, 1000, "running"));
        cancel(&h.id).unwrap();
        assert!(h.is_cancelled());
        h.finish(Err("cancelled".into()));
        let i = list().into_iter().find(|i| i.id == h.id).unwrap();
        assert_eq!(i.state, "cancelled");
        assert!(cancel(&h.id).is_err());
        clear(Some(&h.id));
        assert!(list().iter().all(|i| i.id != h.id));
    }
}
