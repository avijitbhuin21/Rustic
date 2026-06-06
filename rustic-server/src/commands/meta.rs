//! App/meta commands: logs dir, log file listing, frontend error logging.
//! Desktop resolves these via the Tauri app-data dir + the `logging` module;
//! the server resolves them under `data_dir()/logs`.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use sysinfo::{Disks, ProcessesToUpdate, System};

use rustic_app::context::AppContext;

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::ServerContext;

/// One row in the Logs settings list. Mirrors the desktop `LogFileInfo`.
#[derive(Serialize)]
struct LogFileInfo {
    /// Absolute path on disk. Frontend passes this back to `read_log_file`.
    path: String,
    /// Filename only — `rustic.log.YYYY-MM-DD` or `rustic.log` for today's
    /// active file.
    name: String,
    /// Date encoded in the filename (`YYYY-MM-DD`), if it can be parsed.
    /// `None` sorts to the top ("today / unknown").
    date: Option<String>,
    size_bytes: u64,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "get_logs_dir" => ok(ctx.data_dir().join("logs").to_string_lossy().to_string()),
        // confirm_quit is a desktop window-control no-op on the server.
        "confirm_quit" => ok(serde_json::json!(null)),
        "log_frontend_error" => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct A {
                kind: String,
                message: String,
                source: Option<String>,
                stack: Option<String>,
            }
            match parse::<A>(args) {
                Ok(a) => {
                    tracing::error!(
                        target: "rustic::frontend",
                        kind = %a.kind,
                        source = %a.source.unwrap_or_default(),
                        stack = %a.stack.unwrap_or_default(),
                        "frontend error: {}",
                        a.message
                    );
                    ok(serde_json::json!(null))
                }
                Err(e) => Err(e),
            }
        }
        "list_log_files" => list_log_files(ctx),
        "read_log_file" => read_log_file(ctx, args),
        "get_resource_usage" => resource_usage(ctx),
        "get_tunnel_config" => ok(serde_json::json!({
            "previewDomain": std::env::var("RUSTIC_PREVIEW_DOMAIN")
                .ok()
                .map(|s| s.trim().trim_start_matches('.').to_string())
                .filter(|s| !s.is_empty()),
        })),
        _ => return None,
    })
}

/// List the rotating log files in `data_dir()/logs`, newest first.
fn list_log_files(ctx: &ServerContext) -> Result<Value, ApiError> {
    let dir = ctx.data_dir().join("logs");

    let mut out: Vec<LogFileInfo> = Vec::new();
    let read = std::fs::read_dir(&dir)
        .map_err(|e| ApiError::from(format!("Failed to read logs dir {}: {}", dir.display(), e)))?;
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Only surface our own rolling-log files.
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

    // Newest first: bare `rustic.log` (date=None) at the top, then YYYY-MM-DD desc.
    out.sort_by(|a, b| match (&a.date, &b.date) {
        (None, None) => a.name.cmp(&b.name),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => y.cmp(x),
    });

    ok(out)
}

/// Read a single log file. The path is validated to be a regular file inside
/// `data_dir()/logs` — callers cannot use this to slurp arbitrary files.
fn read_log_file(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: PathArg = parse(args)?;
    let log_dir = ctx
        .data_dir()
        .join("logs")
        .canonicalize()
        .map_err(|e| ApiError::from(format!("Cannot canonicalize logs dir: {}", e)))?;

    let target = std::path::Path::new(&a.path)
        .canonicalize()
        .map_err(|e| ApiError::from(format!("Cannot resolve log path: {}", e)))?;

    if !target.starts_with(&log_dir) {
        return Err(ApiError::from(
            "Refused: path is outside the logs directory".to_string(),
        ));
    }
    if !target.is_file() {
        return Err(ApiError::from(
            "Refused: path is not a regular file".to_string(),
        ));
    }

    let content = std::fs::read_to_string(&target)
        .map_err(|e| ApiError::from(format!("Failed to read log file {}: {}", target.display(), e)))?;
    ok(content)
}

/// Resource-monitor snapshot for the web status bar: the server process's
/// resident memory and the storage consumed by the persistent data dir
/// (where all projects/clones live) against the volume's total capacity.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceUsage {
    ram_process_bytes: u64,
    ram_total_bytes: u64,
    disk_used_bytes: u64,
    disk_total_bytes: u64,
}

/// Sample the server process RSS plus the data dir's on-disk size in one shot.
fn resource_usage(ctx: &ServerContext) -> Result<Value, ApiError> {
    let mut sys = System::new();
    let ram_process_bytes = sysinfo::get_current_pid()
        .ok()
        .and_then(|pid| {
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            sys.process(pid).map(|p| p.memory())
        })
        .unwrap_or(0);

    sys.refresh_memory();
    let ram_total_bytes = sys.total_memory();

    let data_dir = ctx.data_dir();
    let disk_total_bytes = volume_total_bytes(&data_dir);
    let disk_used_bytes = data_dir_size_cached(&data_dir);

    ok(ResourceUsage {
        ram_process_bytes,
        ram_total_bytes,
        disk_used_bytes,
        disk_total_bytes,
    })
}

/// Total capacity (bytes) of the disk volume that contains `path`, picking the
/// mount point that is the longest matching prefix. Falls back to the first
/// listed disk when nothing matches.
fn volume_total_bytes(path: &Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if path.starts_with(mount) {
            let len = mount.as_os_str().len();
            if best.map_or(true, |(prev, _)| len > prev) {
                best = Some((len, disk.total_space()));
            }
        }
    }
    best.map(|(_, total)| total)
        .or_else(|| disks.list().first().map(|d| d.total_space()))
        .unwrap_or(0)
}

/// On-disk byte size of the data dir, cached for 60s. Walking the full tree on
/// every 5s status-bar poll would hammer the disk (projects carry node_modules
/// / .git), so we recompute at most once a minute.
fn data_dir_size_cached(path: &Path) -> u64 {
    static CACHE: OnceLock<Mutex<Option<(Instant, u64)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, size)) = *cache.lock().unwrap() {
        if at.elapsed() < Duration::from_secs(60) {
            return size;
        }
    }
    let size = dir_size(path);
    *cache.lock().unwrap() = Some((Instant::now(), size));
    size
}

/// Sum the bytes of every regular file under `root` (iterative, symlinks
/// skipped to avoid cycles). Unreadable entries are ignored.
fn dir_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}