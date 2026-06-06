//! `BrowserManager` — the single Chromium instance's lifecycle + process control.
//!
//! Strict "nothing runs when closed" contract (from the plan): when no browser
//! window is open there must be **zero** Chromium processes. Chromium is spawned
//! lazily on first open and fully terminated — process group and all renderer
//! children — when the window closes, the last tab closes, the owning CDP
//! sockets all drop (after a short grace), or the server shuts down.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use rustic_app::context::{EventEmitter, EventEmitterExt};
use serde_json::json;

use super::cdp;

/// Default CDP port. Loopback-only; never published. Overridable for tests via
/// `RUSTIC_BROWSER_DEBUG_PORT`.
const DEFAULT_DEBUG_PORT: u16 = 9222;
/// Max time to wait for Chromium to answer `/json/version` after spawn.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long Chromium may have **zero** client CDP sockets before the watchdog
/// reaps it. Covers both "window closed" and "opened but never connected".
const IDLE_GRACE: Duration = Duration::from_secs(12);
/// Watchdog poll cadence.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(3);

/// The host-side CDP endpoints for a running Chromium.
#[derive(Clone, Debug)]
pub struct CdpEndpoint {
    /// e.g. `http://127.0.0.1:9222`
    pub http_base: String,
    /// The browser-level CDP WebSocket (`Target.*` / `Browser.*`).
    pub browser_ws: String,
}

struct Inner {
    running: bool,
    child: Option<Child>,
    endpoint: Option<CdpEndpoint>,
    /// `Some(t)` while running with no active client sockets — the watchdog
    /// reaps once `t.elapsed() >= IDLE_GRACE`. `None` while a socket is live.
    idle_since: Option<Instant>,
}

pub struct BrowserManager {
    inner: tokio::sync::Mutex<Inner>,
    data_dir: PathBuf,
    port: u16,
    emitter: Arc<dyn EventEmitter>,
    /// Count of live client CDP proxy sockets (viewport + DevTools).
    socket_count: AtomicUsize,
    watchdog_started: AtomicBool,
}

impl BrowserManager {
    pub fn new(data_dir: PathBuf, emitter: Arc<dyn EventEmitter>) -> Self {
        let port = std::env::var("RUSTIC_BROWSER_DEBUG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_DEBUG_PORT);
        Self {
            inner: tokio::sync::Mutex::new(Inner {
                running: false,
                child: None,
                endpoint: None,
                idle_since: None,
            }),
            data_dir,
            port,
            emitter,
            socket_count: AtomicUsize::new(0),
            watchdog_started: AtomicBool::new(false),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Build the page-level CDP WebSocket URL for a target id.
    pub fn page_ws(&self, target_id: &str) -> String {
        format!("ws://127.0.0.1:{}/devtools/page/{target_id}", self.port)
    }

    /// The current endpoint if Chromium is running, else `None`.
    pub async fn endpoint_if_running(&self) -> Option<CdpEndpoint> {
        let inner = self.inner.lock().await;
        if inner.running {
            inner.endpoint.clone()
        } else {
            None
        }
    }

    /// Idempotent spawn. If Chromium is already up (and the process is still
    /// alive) returns the existing endpoint; otherwise launches it, waits for
    /// the CDP port, and starts the idle watchdog. Holds the lock for the whole
    /// startup so a concurrent open or a teardown can't race the port.
    pub async fn ensure_started(self: &Arc<Self>) -> Result<CdpEndpoint, String> {
        let mut inner = self.inner.lock().await;

        if inner.running {
            let alive = inner
                .child
                .as_mut()
                .map(|c| matches!(c.try_wait(), Ok(None)))
                .unwrap_or(false);
            if alive {
                if let Some(ep) = &inner.endpoint {
                    return Ok(ep.clone());
                }
            }
            // Stale/dead — fall through and respawn from a clean slate.
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            inner.running = false;
            inner.endpoint = None;
        }

        let bin = cdp::find_chromium().ok_or_else(|| {
            "No Chromium/Chrome binary found. Install `chromium` (the container \
             image does) or set CHROME_BIN to its path."
                .to_string()
        })?;

        let profile = self.data_dir.join("browser-profile");
        std::fs::create_dir_all(&profile)
            .map_err(|e| format!("cannot create browser profile dir: {e}"))?;

        let mut child = spawn_chromium(&bin, self.port, &profile)
            .map_err(|e| format!("failed to spawn Chromium ({}): {e}", bin.display()))?;

        let http_base = format!("http://127.0.0.1:{}", self.port);
        if let Err(e) = wait_until_ready(&http_base, &mut child).await {
            // Startup failed — make sure we don't leak the half-spawned process.
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }

        let browser_ws = match cdp::browser_ws_url(&http_base).await {
            Ok(ws) => ws,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };

        let endpoint = CdpEndpoint {
            http_base,
            browser_ws,
        };
        inner.child = Some(child);
        inner.endpoint = Some(endpoint.clone());
        inner.running = true;
        // Start the idle clock immediately: an open that never gets a client
        // socket still gets reaped after the grace window.
        inner.idle_since = Some(Instant::now());
        drop(inner);

        self.ensure_watchdog();
        tracing::info!(port = self.port, "Chromium started");
        Ok(endpoint)
    }

    /// Gracefully close then group-kill Chromium and reap it. Idempotent.
    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        if !inner.running && inner.child.is_none() {
            return;
        }
        let endpoint = inner.endpoint.take();
        let child = inner.child.take();
        inner.running = false;
        inner.idle_since = None;

        // Graceful: ask Chromium to close itself. Best-effort, short timeout.
        if let Some(ep) = &endpoint {
            let _ = tokio::time::timeout(Duration::from_secs(2), cdp::browser_close(&ep.browser_ws))
                .await;
        }
        // Hard teardown of the whole process group on a blocking thread (it
        // sleeps between TERM and KILL). Still holding the lock so no respawn
        // can race the port mid-kill.
        if let Some(mut child) = child {
            let _ = tokio::task::spawn_blocking(move || terminate_group(&mut child)).await;
        }
        drop(inner);

        tracing::info!("Chromium stopped");
        self.emitter.emit("browser-stopped", json!({}));
    }

    /// A client CDP proxy socket connected. Cancels any pending idle teardown.
    pub async fn socket_opened(&self) {
        self.socket_count.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.inner.lock().await;
        inner.idle_since = None;
    }

    /// A client CDP proxy socket dropped. When the last one goes, arm the idle
    /// clock so the watchdog tears Chromium down after the grace window.
    pub async fn socket_closed(&self) {
        let prev = self.socket_count.fetch_sub(1, Ordering::SeqCst);
        if prev <= 1 {
            let mut inner = self.inner.lock().await;
            if inner.running {
                inner.idle_since = Some(Instant::now());
            }
        }
    }

    /// Spawn the idle watchdog exactly once. Holds only a `Weak` ref so the
    /// task exits naturally if the manager (and thus the server) is dropped.
    fn ensure_watchdog(self: &Arc<Self>) {
        if self.watchdog_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak: Weak<Self> = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(WATCHDOG_INTERVAL).await;
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let should_stop = {
                    let inner = this.inner.lock().await;
                    inner.running
                        && this.socket_count.load(Ordering::SeqCst) == 0
                        && inner
                            .idle_since
                            .map(|t| t.elapsed() >= IDLE_GRACE)
                            .unwrap_or(false)
                };
                if should_stop {
                    tracing::info!("idle watchdog reaping Chromium (no client sockets)");
                    this.stop().await;
                }
            }
        });
    }
}

/// Launch Chromium headless-new with the loopback-bound remote-debugging port.
/// On Unix the child leads its own process group so a group-kill reaps every
/// renderer/zygote child too.
fn spawn_chromium(bin: &std::path::Path, port: u16, profile: &std::path::Path) -> std::io::Result<Child> {
    let mut cmd = Command::new(bin);
    cmd.arg("--headless=new")
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--window-size=1280,800")
        // An initial page so `/json` has a target the moment the port is up.
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New process group with the child as leader (pgid == child pid), so
        // `kill -<sig> -<pid>` signals the whole tree.
        cmd.process_group(0);
    }

    cmd.spawn()
}

/// Remove stale Chromium singleton lock files left by an unclean shutdown so a
/// fresh launch against the persisted profile doesn't exit immediately.
fn clean_singleton_locks(profile: &std::path::Path) {
    for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(name));
    }
}

/// Read the last ~2KB of a log file (the Chromium stderr log) for surfacing in
/// a startup-failure error. Returns an empty string if it can't be read.
fn read_log_tail(path: &std::path::Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    const MAX: u64 = 2048;
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if f.seek(SeekFrom::Start(len.saturating_sub(MAX))).is_err() {
        return String::new();
    }
    let mut buf = String::new();
    let _ = f.read_to_string(&mut buf);
    buf.trim().to_string()
}

/// Poll until Chromium answers `/json/version`, or it exits, or we time out.
async fn wait_until_ready(http_base: &str, child: &mut Child) -> Result<(), String> {
    let version_url = format!("{http_base}/json/version");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("Chromium exited during startup ({status})"));
        }
        if let Ok(resp) = client
            .get(&version_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err("Chromium did not open its CDP port in time".to_string());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// TERM the process group, wait briefly, then KILL it, then reap. On non-Unix
/// (local dev) fall back to killing just the child. Runs on a blocking thread.
fn terminate_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        // pgid == pid (we spawned with process_group(0)). Negative pid → group.
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{pid}"))
            .status();
        for _ in 0..20 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{pid}"))
            .status();
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}
