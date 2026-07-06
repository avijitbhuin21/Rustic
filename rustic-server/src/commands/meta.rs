//! App/meta commands: logs dir, log file listing, frontend error logging.
//! Desktop resolves these via the Tauri app-data dir + the `logging` module;
//! the server resolves them under `data_dir()/logs`.

use std::path::Path;

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

    let content = std::fs::read_to_string(&target).map_err(|e| {
        ApiError::from(format!(
            "Failed to read log file {}: {}",
            target.display(),
            e
        ))
    })?;
    ok(content)
}

/// Resource-monitor snapshot for the web status bar: live memory + storage use,
/// cheap enough to poll every couple of seconds.
///
/// On Linux containers (Railway) both figures come straight from the kernel —
/// cgroup memory accounting (the WHOLE container: the server PLUS every child it
/// spawns — Chromium, node, cargo, …) and the data volume's filesystem usage —
/// so they move in real time as work starts/stops, with no expensive tree walk.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceUsage {
    /// Memory currently in use (whole container where cgroups exist).
    ram_process_bytes: u64,
    /// Memory ceiling (cgroup limit, else host total).
    ram_total_bytes: u64,
    /// Bytes used on the data volume.
    disk_used_bytes: u64,
    /// Total capacity of the data volume.
    disk_total_bytes: u64,
}

fn resource_usage(ctx: &ServerContext) -> Result<Value, ApiError> {
    let (ram_used, ram_total) = memory_usage();
    let (disk_used, disk_total) = disk_usage(&ctx.data_dir());
    ok(ResourceUsage {
        ram_process_bytes: ram_used,
        ram_total_bytes: ram_total,
        disk_used_bytes: disk_used,
        disk_total_bytes: disk_total,
    })
}

/// (used, total) memory in bytes. Prefers cgroup accounting so the figure
/// reflects the entire container — including spawned Chromium/node/dev-servers —
/// and updates live; falls back to this process's RSS + host total off-cgroup.
fn memory_usage() -> (u64, u64) {
    if let Some(pair) = cgroup_memory() {
        return pair;
    }
    let mut sys = System::new();
    let used = sysinfo::get_current_pid()
        .ok()
        .and_then(|pid| {
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            sys.process(pid).map(|p| p.memory())
        })
        .unwrap_or(0);
    sys.refresh_memory();
    (used, sys.total_memory())
}

/// Host physical memory — the cgroup-limit fallback when a container is
/// unconstrained.
#[cfg(target_os = "linux")]
fn host_total_memory() -> u64 {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// Container memory (used, total) from the cgroup, or `None` when not in one.
/// "used" is the working set (current minus reclaimable file cache), matching
/// how container tooling reports memory; "total" is the limit, or the host total
/// when the cgroup is unconstrained (`max` / the v1 unlimited sentinel).
#[cfg(target_os = "linux")]
fn cgroup_memory() -> Option<(u64, u64)> {
    // cgroup v2
    if let Ok(cur) = std::fs::read_to_string("/sys/fs/cgroup/memory.current") {
        let current = cur.trim().parse::<u64>().ok()?;
        let inactive =
            cgroup_stat_field("/sys/fs/cgroup/memory.stat", "inactive_file").unwrap_or(0);
        let used = current.saturating_sub(inactive);
        let total = std::fs::read_to_string("/sys/fs/cgroup/memory.max")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or_else(host_total_memory);
        return Some((used, total));
    }
    // cgroup v1
    if let Ok(cur) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.usage_in_bytes") {
        let current = cur.trim().parse::<u64>().ok()?;
        let inactive =
            cgroup_stat_field("/sys/fs/cgroup/memory/memory.stat", "total_inactive_file")
                .unwrap_or(0);
        let used = current.saturating_sub(inactive);
        let total = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            // v1 "unlimited" is a near-u64::MAX sentinel — treat as unconstrained.
            .filter(|&l| l < (1u64 << 62))
            .unwrap_or_else(host_total_memory);
        return Some((used, total));
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn cgroup_memory() -> Option<(u64, u64)> {
    None
}

/// Read a single `key value` field (bytes) from a cgroup stat file.
#[cfg(target_os = "linux")]
fn cgroup_stat_field(path: &str, key: &str) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some(key) {
            return it.next().and_then(|v| v.parse::<u64>().ok());
        }
    }
    None
}

/// (used, total) bytes of the volume holding `path`, read from the filesystem —
/// O(1) and real-time, so storage updates live without the old 60s tree-walk
/// cache. Picks the longest matching mount prefix; falls back to the first disk.
fn disk_usage(path: &Path) -> (u64, u64) {
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if path.starts_with(mount) {
            let len = mount.as_os_str().len();
            if best.map_or(true, |(prev, _, _)| len > prev) {
                best = Some((len, disk.total_space(), disk.available_space()));
            }
        }
    }
    let (total, available) = best
        .map(|(_, t, a)| (t, a))
        .or_else(|| {
            disks
                .list()
                .first()
                .map(|d| (d.total_space(), d.available_space()))
        })
        .unwrap_or((0, 0));
    (total.saturating_sub(available), total)
}
