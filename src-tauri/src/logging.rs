//! Persistent file logging with daily rotation and 7-day retention.
//!
//! Writes to `<app_data_dir>/logs/rustic.log.YYYY-MM-DD` (no console in
//! release builds). A panic hook routes panics through `tracing::error!`
//! to the same file.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::NaiveDate;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

const LOG_FILE_PREFIX: &str = "rustic.log";
const RETENTION_DAYS: i64 = 7;

/// `tracing_appender::non_blocking` returns a guard that flushes the background
/// writer thread when dropped. The guard must outlive every `tracing::*` call
/// in the program — drop it early and any pending log line is silently lost.
/// Stash it in a process-wide `OnceLock` so it lives for as long as the app.
static FILE_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Path to the active logs directory. Set inside `init` so the
/// `get_logs_dir` Tauri command can return it without re-resolving.
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Initialise the global tracing subscriber with both a stderr layer (visible
/// in `cargo run` / Tauri dev) and a daily-rotating file layer (visible in
/// release where there is no console).
///
/// Idempotent in the sense that the `OnceLock` guards prevent a second call
/// from leaking a writer thread, but `Registry::init` will return Err on a
/// second call — we ignore that so callers can retry safely.
pub fn init(app_data_dir: &Path) -> std::io::Result<PathBuf> {
    let log_dir = app_data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    // Best-effort cleanup of stale logs. A failure here should never block
    // startup — losing a day of logs is bad, but not booting is worse.
    if let Err(e) = cleanup_old_logs(&log_dir, RETENTION_DAYS) {
        eprintln!("[logging] cleanup_old_logs failed: {}", e);
    }

    let file_appender = rolling::daily(&log_dir, LOG_FILE_PREFIX);
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    // Park the guard for the lifetime of the process. Ignore the error case
    // where init() was somehow called twice — the previous guard is still in
    // place and the second call's WorkerGuard will be dropped here, flushing
    // its (empty) buffer cleanly.
    let _ = FILE_GUARD.set(guard);
    let _ = LOG_DIR.set(log_dir.clone());

    let filter = EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| {
        EnvFilter::new("info,reqwest=warn,hyper=warn,tower=warn,h2=warn,rustls=warn")
    });

    let file_layer = fmt::layer()
        .with_writer(file_writer)
        .with_target(true)
        .with_thread_ids(true)
        .with_ansi(false);

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(false);

    // Best-effort: ignore the SetGlobalDefaultError that fires if a previous
    // call already installed a subscriber — the file logger will simply not
    // be wired up that second time, but the process still works.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    install_panic_hook();

    let date = chrono::Local::now().format("%Y-%m-%d");
    tracing::info!(
        target: "rustic::startup",
        log_dir = %log_dir.display(),
        date = %date,
        retention_days = RETENTION_DAYS,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        version = env!("CARGO_PKG_VERSION"),
        "logging initialised"
    );

    Ok(log_dir)
}

/// Tauri command target: returns the directory containing the rolling log
/// files, so the frontend can offer "Reveal logs folder" or, later, an
/// opt-in "Send logs to support" flow.
pub fn current_log_dir() -> Option<PathBuf> {
    LOG_DIR.get().cloned()
}

/// Install a panic hook that routes panics through `tracing::error!` so they
/// land in the rotating log file. Without this, a panic in a background
/// thread or a Tauri command body would only print to stderr — invisible in
/// a release build where the GUI subsystem suppresses the console.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());

        let payload: &str = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.as_str()
        } else {
            "<non-string panic payload>"
        };

        let backtrace = std::backtrace::Backtrace::force_capture();

        tracing::error!(
            target: "rustic::panic",
            location = %location,
            payload = %payload,
            backtrace = %backtrace,
            "thread panicked"
        );

        // Still call the previous (default) hook so libraries that depend on
        // its behaviour — e.g. abort-on-panic in test runners — keep working.
        prev(info);
    }));
}

/// Remove `rustic.log.YYYY-MM-DD` files whose embedded date is older than
/// `keep_days` days. Files we can't parse are left alone so a stray file the
/// user dropped in the directory doesn't get clobbered.
fn cleanup_old_logs(log_dir: &Path, keep_days: i64) -> std::io::Result<()> {
    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(keep_days);

    for entry in std::fs::read_dir(log_dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // Daily rotator names look like `rustic.log.2026-05-07`. Strip the
        // prefix + dot and try to parse the rest as a date.
        let suffix = match file_name.strip_prefix(&format!("{}.", LOG_FILE_PREFIX)) {
            Some(s) => s,
            None => continue,
        };
        let date = match NaiveDate::parse_from_str(suffix, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => continue,
        };

        if date < cutoff {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("[logging] failed to delete stale log {}: {}", path.display(), e);
            }
        }
    }
    Ok(())
}
