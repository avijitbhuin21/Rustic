//! Process / task-manager commands (server-only).
//!
//! `list_processes` enumerates everything running in the container's PID
//! namespace (so it shows this VM's processes and nothing from other tenants),
//! and `kill_process` terminates one by PID. A small set of PIDs is *protected*
//! — refused server-side, not just hidden in the UI — so the task manager can
//! never take the box down: PID 1 (the container entrypoint), this server and
//! its whole ancestor chain, kernel threads (`kthreadd` + descendants), and a
//! name denylist of core daemons.
//!
//! CPU% is a delta between two refreshes, so we keep one long-lived `System`
//! sampler refreshed on every poll; the first sample reads 0 and subsequent
//! ones are meaningful.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use serde_json::{json, Value};
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    _ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "list_processes" => list_processes(),
        "kill_process" => kill_process(args),
        _ => return None,
    })
}

/// Core daemons that should never be killable even if they aren't PID 1 / an
/// ancestor. Matched case-insensitively against the process name.
const PROTECTED_NAMES: &[&str] = &[
    "init",
    "systemd",
    "dockerd",
    "containerd",
    "containerd-shim",
    "runc",
    "tini",
    "dumb-init",
    "s6-svscan",
    "sshd",
    "kthreadd",
];

/// One row in the task manager.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessRow {
    pid: u32,
    ppid: u32,
    name: String,
    /// Full command line (truncated), for disambiguating identical names.
    cmd: String,
    cpu_percent: f32,
    memory_bytes: u64,
    /// True → killing this would risk the VM; the UI flags it red and disables
    /// the kill button, and `kill_process` refuses it.
    protected: bool,
}

/// The long-lived sampler. Reused across polls so per-process CPU% (a delta
/// since the previous refresh) is meaningful from the second poll on.
fn sampler() -> &'static Mutex<System> {
    static SAMPLER: OnceLock<Mutex<System>> = OnceLock::new();
    SAMPLER.get_or_init(|| Mutex::new(System::new()))
}

fn list_processes() -> Result<Value, ApiError> {
    let mut sys = sampler()
        .lock()
        .map_err(|_| ApiError::from("process sampler lock poisoned".to_string()))?;
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let protected = protected_pids(&sys);

    let mut rows: Vec<ProcessRow> = sys
        .processes()
        .iter()
        .map(|(pid, p)| {
            let pid_u = pid.as_u32();
            ProcessRow {
                pid: pid_u,
                ppid: p.parent().map(|pp| pp.as_u32()).unwrap_or(0),
                name: p.name().to_string_lossy().to_string(),
                cmd: truncate(&join_cmd(p), 240),
                cpu_percent: (p.cpu_usage() * 10.0).round() / 10.0,
                memory_bytes: p.memory(),
                protected: protected.contains(&pid_u),
            }
        })
        .collect();

    // Heaviest first — that's what the operator wants to reclaim.
    rows.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
    ok(rows)
}

fn kill_process(args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        pid: u32,
        /// SIGKILL when true; SIGTERM (graceful) otherwise.
        #[serde(default)]
        force: bool,
    }
    let a: A = parse(args)?;

    let mut sys = sampler()
        .lock()
        .map_err(|_| ApiError::from("process sampler lock poisoned".to_string()))?;
    sys.refresh_processes(ProcessesToUpdate::All, true);

    // Recompute protection from the live snapshot — never trust the client.
    if protected_pids(&sys).contains(&a.pid) {
        return Err(ApiError {
            status: 403,
            message: "Refusing to kill a protected system process".to_string(),
        });
    }

    let proc = sys
        .process(Pid::from_u32(a.pid))
        .ok_or_else(|| ApiError::from(format!("No process with pid {}", a.pid)))?;

    // SIGTERM by default; fall back to SIGKILL if the platform doesn't support
    // the signal (Windows → `kill_with` returns None).
    let killed = if a.force {
        proc.kill()
    } else {
        proc.kill_with(Signal::Term).unwrap_or_else(|| proc.kill())
    };

    if killed {
        tracing::info!(pid = a.pid, force = a.force, "task manager killed process");
        ok(json!({ "ok": true }))
    } else {
        Err(ApiError::from(format!(
            "Failed to signal pid {} (insufficient permission or already gone)",
            a.pid
        )))
    }
}

/// Compute the set of PIDs that must not be killed.
fn protected_pids(sys: &System) -> HashSet<u32> {
    let mut protected: HashSet<u32> = HashSet::new();

    // PID 1 — the container entrypoint.
    protected.insert(1);

    // This server + its whole ancestor chain (so the task manager can't kill
    // the thing serving it). Guard against cycles with a visited cap.
    if let Ok(me) = sysinfo::get_current_pid() {
        let mut cur = Some(me);
        let mut hops = 0;
        while let Some(p) = cur {
            if !protected.insert(p.as_u32()) || hops > 64 {
                break;
            }
            cur = sys.process(p).and_then(|pr| pr.parent());
            hops += 1;
        }
    }

    // Kernel threads: kthreadd (PID 2) and everything parented under it.
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, p) in sys.processes() {
        if let Some(pp) = p.parent() {
            children.entry(pp.as_u32()).or_default().push(pid.as_u32());
        }
    }
    let mut stack = vec![2u32];
    while let Some(p) = stack.pop() {
        if protected.insert(p) {
            if let Some(kids) = children.get(&p) {
                stack.extend(kids.iter().copied());
            }
        }
    }

    // Name denylist backstop.
    for (pid, p) in sys.processes() {
        let name = p.name().to_string_lossy().to_ascii_lowercase();
        if PROTECTED_NAMES.iter().any(|n| name == *n) {
            protected.insert(pid.as_u32());
        }
    }

    protected
}

fn join_cmd(p: &sysinfo::Process) -> String {
    let parts: Vec<String> = p
        .cmd()
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    if parts.is_empty() {
        p.name().to_string_lossy().to_string()
    } else {
        parts.join(" ")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}
