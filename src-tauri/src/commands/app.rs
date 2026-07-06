use serde::Serialize;
use tauri::{AppHandle, Manager};

/// Return the absolute path to the rotating-log directory, so the frontend
/// can offer "Reveal logs folder" or, with explicit user consent, attach the
/// logs to a support / crash report.
#[tauri::command]
pub fn get_logs_dir() -> Result<String, String> {
    crate::logging::current_log_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Logging is not initialised".to_string())
}

/// One row in the Logs settings list.
#[derive(Serialize)]
pub struct LogFileInfo {
    /// Absolute path on disk. Frontend passes this back to `read_log_file`.
    pub path: String,
    /// Filename only — `rustic.log.YYYY-MM-DD` or `rustic.log` for today's
    /// active file (the rolling appender writes to the bare name until the
    /// first roll, then to `<name>.<date>`; depending on tracing-appender
    /// version both forms can appear).
    pub name: String,
    /// Date encoded in the filename (`YYYY-MM-DD`), if it can be parsed.
    /// `None` means "today / unknown" and the row should sort to the top.
    pub date: Option<String>,
    pub size_bytes: u64,
}

/// List the rotating log files currently on disk, newest first. Used by the
/// Settings → Logs panel so the user can pick a day and open it in the editor.
#[tauri::command]
pub fn list_log_files() -> Result<Vec<LogFileInfo>, String> {
    let dir = crate::logging::current_log_dir()
        .ok_or_else(|| "Logging is not initialised".to_string())?;

    let mut out: Vec<LogFileInfo> = Vec::new();
    let read = std::fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read logs dir {}: {}", dir.display(), e))?;
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Only surface our own rolling-log files; ignore stray content the
        // user may have dropped in the dir.
        let is_ours = name == "rustic.log" || name.starts_with("rustic.log.");
        if !is_ours {
            continue;
        }
        let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let date = name
            .strip_prefix("rustic.log.")
            .filter(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok())
            .map(|s| s.to_string());

        out.push(LogFileInfo {
            path: path.to_string_lossy().to_string(),
            name,
            date,
            size_bytes,
        });
    }

    // Sort newest first: bare `rustic.log` (today's active, date=None) at the
    // top, then YYYY-MM-DD descending.
    out.sort_by(|a, b| match (&a.date, &b.date) {
        (None, None) => a.name.cmp(&b.name),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => y.cmp(x),
    });

    Ok(out)
}

/// Read a single log file's contents. The path is validated to be a regular
/// file inside the active logs directory — callers cannot use this command
/// to slurp arbitrary files off disk, even though the bytes themselves are
/// plain text (defence-in-depth: an XSS-via-log-line attacker shouldn't be
/// able to pivot to reading `~/.aws/credentials`).
#[tauri::command]
pub fn read_log_file(path: String) -> Result<String, String> {
    let log_dir = crate::logging::current_log_dir()
        .ok_or_else(|| "Logging is not initialised".to_string())?;
    let log_dir = log_dir
        .canonicalize()
        .map_err(|e| format!("Cannot canonicalize logs dir: {}", e))?;

    let target = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve log path: {}", e))?;

    if !target.starts_with(&log_dir) {
        return Err("Refused: path is outside the logs directory".to_string());
    }
    if !target.is_file() {
        return Err("Refused: path is not a regular file".to_string());
    }

    std::fs::read_to_string(&target)
        .map_err(|e| format!("Failed to read log file {}: {}", target.display(), e))
}

/// Persist a frontend error/crash to the rolling log file. The webview's
/// `window.onerror` / `unhandledrejection` handlers and the React error
/// boundary call this so renderer-side failures — which never reach the Rust
/// panic hook and are invisible once the webview tears down — are captured in
/// the same log the backend writes to. `kind` is e.g. "error",
/// "unhandledrejection", "react-error-boundary".
#[tauri::command]
pub fn log_frontend_error(
    kind: String,
    message: String,
    source: Option<String>,
    stack: Option<String>,
) {
    tracing::error!(
        target: "rustic::frontend",
        kind = %kind,
        source = %source.unwrap_or_default(),
        stack = %stack.unwrap_or_default(),
        "frontend error: {}",
        message
    );
}

/// Quit the app without further prompting. The frontend calls this after
/// it has confirmed that any dirty buffers are saved/discarded.
#[tauri::command]
pub fn confirm_quit(app: AppHandle) {
    // Best-effort WAL truncate so the -wal sidecar doesn't grow unbounded
    // across sessions. Failures here are non-fatal; the next launch will
    // simply pick up the existing -wal.
    if let Some(state) = app.try_state::<crate::state::AppState>() {
        if let Ok(db) = state.db.lock() {
            let _ = db.checkpoint_truncate();
        }
    }
    app.exit(0);
}
