//! Power / session-lifecycle commands (server-only).
//!
//! `power_off` is the "soft power button": it flushes every RAM/CPU-consuming
//! background process the server spun up — agent tasks, MCP servers, terminal
//! process trees (the dev servers `node`/`cargo`/`air`/`go` spawned inside a
//! shell, not just the shell), the embedded Chromium, and Cloudflare tunnels —
//! and then bumps the session generation so every outstanding auth token dies.
//! Persistent on-disk data (the project files / DB) is untouched: this reclaims
//! RAM while the box sits idle, it does not delete anything.
//!
//! It is driven from the web status-bar "power" button (with a confirm) and the
//! idle auto-logout timer. `get_power_config` / `set_power_config` persist the
//! idle settings (keep-alive + idle timeout) the frontend reads.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rustic_app::context::{AppContext, EventEmitterExt};
use rustic_app::sync_ext::MutexExt;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "power_off" => power_off(ctx).await,
        "get_power_config" => get_power_config(ctx),
        "set_power_config" => set_power_config(ctx, args),
        _ => return None,
    })
}

/// Idle / keep-alive settings, persisted under the `power_config` DB key and
/// read by the web frontend's idle-logout timer.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PowerConfig {
    /// When true, the idle auto-logout timer is disabled entirely — the session
    /// stays logged in indefinitely regardless of activity. When false, the
    /// frontend powers off after `idle_timeout_minutes` of no user activity.
    #[serde(default)]
    keep_alive: bool,
    /// Minutes of inactivity before the idle auto-logout fires (ignored when
    /// `keep_alive` is true).
    #[serde(default = "default_idle_minutes")]
    idle_timeout_minutes: u32,
}

fn default_idle_minutes() -> u32 {
    10
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            keep_alive: false,
            idle_timeout_minutes: default_idle_minutes(),
        }
    }
}

fn read_power_config(ctx: &ServerContext) -> PowerConfig {
    ctx.state()
        .db
        .lock_safe()
        .get_setting("power_config")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<PowerConfig>(&j).ok())
        .unwrap_or_default()
}

fn get_power_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let pc = read_power_config(ctx);
    ok(json!({
        "keepAlive": pc.keep_alive,
        "idleTimeoutMinutes": pc.idle_timeout_minutes,
    }))
}

fn set_power_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        #[serde(default)]
        keep_alive: bool,
        #[serde(default = "default_idle_minutes")]
        idle_timeout_minutes: u32,
    }
    let a: A = parse(args)?;
    // Clamp to a sane floor so a 0 can't make the idle timer fire instantly.
    let pc = PowerConfig {
        keep_alive: a.keep_alive,
        idle_timeout_minutes: a.idle_timeout_minutes.max(1),
    };
    let json = serde_json::to_string(&pc).map_err(|e| e.to_string())?;
    ctx.state()
        .db
        .lock_safe()
        .set_setting("power_config", &json)
        .map_err(|e| e.to_string())?;
    ok(json!({
        "keepAlive": pc.keep_alive,
        "idleTimeoutMinutes": pc.idle_timeout_minutes,
    }))
}

/// Flush everything, then invalidate all sessions. Returns a small summary of
/// what was reclaimed (handy for the logs and a future "powered off" toast).
async fn power_off(ctx: &ServerContext) -> Result<Value, ApiError> {
    tracing::info!("power_off: flushing all background processes");

    let terminals = flush_terminals(ctx);
    flush_agents(ctx);
    ctx.browser.stop().await;
    let tunnels = ctx.cloudflared.close_all().await;

    // Best-effort: hint the allocator to return freed heap pages to the OS so
    // the idle RSS floor drops as far as it can after the children are gone.
    malloc_trim();

    // Invalidate every outstanding token by bumping the generation, and persist
    // it so the logout survives a restart.
    let new_gen = ctx.session_gen.fetch_add(1, Ordering::SeqCst) + 1;
    if let Err(e) = ctx
        .state()
        .db
        .lock_safe()
        .set_setting("session_generation", &new_gen.to_string())
    {
        tracing::warn!("power_off: failed to persist session generation: {e}");
    }

    tracing::info!(
        terminals,
        tunnels,
        session_generation = new_gen,
        "power_off: flush complete, all sessions invalidated"
    );

    ok(json!({
        "ok": true,
        "terminalsKilled": terminals,
        "tunnelsClosed": tunnels,
    }))
}

/// Kill every terminal session's full process tree (so dev servers spawned
/// inside a shell die too, not just the shell), then clear the manager.
/// Returns the number of sessions reclaimed.
fn flush_terminals(ctx: &ServerContext) -> usize {
    let sessions: Vec<(u64, Option<u32>)> = {
        let mgr = ctx.state().terminal_manager.lock_safe();
        mgr.list_sessions().iter().map(|s| (s.id, s.pid)).collect()
    };
    for (_, pid) in &sessions {
        if let Some(pid) = pid {
            kill_process_tree(*pid);
        }
    }
    {
        let mut mgr = ctx.state().terminal_manager.lock_safe();
        for (id, _) in &sessions {
            mgr.destroy_session(*id);
        }
    }
    if !sessions.is_empty() {
        ctx.emit("terminal-list-changed", ());
    }
    sessions.len()
}

/// Cancel and drop every agent task, then disconnect all MCP servers (killing
/// their child processes). The executor threads hold their own clone of each
/// cancellation token, so flipping it before clearing the maps lets them
/// observe the cancel and unwind.
fn flush_agents(ctx: &ServerContext) {
    let agent_arc = ctx.state().agent.clone();
    let mut agent = agent_arc.lock_safe();
    for token in agent.cancellation_tokens.values() {
        token.store(true, Ordering::SeqCst);
    }
    agent.cancellation_tokens.clear();
    agent.tasks.clear();
    let mcp = agent.mcp_manager.clone();
    drop(agent);
    mcp.lock_safe().disconnect_all();
}

/// Best-effort kill of `pid` and every descendant — reclaiming the RAM of dev
/// servers (`node`/`cargo`/`air`/`go`) the shell spawned, not just the shell.
fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    unix_kill_tree(pid);
    #[cfg(windows)]
    {
        // `/T` kills the whole tree rooted at the PID; `/F` forces it.
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(unix)]
fn unix_kill_tree(root: u32) {
    use std::collections::HashMap;

    // Build a ppid → children map from /proc, then BFS the whole subtree rooted
    // at `root` before signalling anything (so a process re-parenting to init
    // mid-kill can't drop out of the set).
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for entry in rd.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            if let Some(ppid) = read_ppid(pid) {
                children.entry(ppid).or_default().push(pid);
            }
        }
    }

    let mut victims = vec![root];
    let mut i = 0;
    while i < victims.len() {
        let p = victims[i];
        if let Some(kids) = children.get(&p) {
            for &k in kids {
                if !victims.contains(&k) {
                    victims.push(k);
                }
            }
        }
        i += 1;
    }

    // SIGKILL the collected pids in one shot, then also signal the shell's
    // process group (portable-pty gives the shell its own session/group, so
    // `-root` sweeps up anything that set its own pgid underneath it).
    let mut args: Vec<String> = vec!["-KILL".to_string()];
    args.extend(victims.iter().map(|p| p.to_string()));
    let _ = std::process::Command::new("kill").args(&args).status();
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &format!("-{root}")])
        .status();
}

/// Read the parent pid from `/proc/<pid>/stat`. The `comm` field is wrapped in
/// parens and may itself contain spaces/parens, so we anchor on the LAST ')':
/// the fields after it are `state ppid ...`.
#[cfg(unix)]
fn read_ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rparen = stat.rfind(')')?;
    let rest = stat.get(rparen + 1..)?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse::<u32>().ok()
}

/// Ask glibc to return freed heap pages to the OS. No-op off glibc/Linux (musl,
/// macOS, Windows don't expose `malloc_trim`); the dominant reclaim is the
/// killed child processes regardless.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn malloc_trim() {
    extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    // SAFETY: malloc_trim is a thread-safe glibc allocator hint with no
    // preconditions; the return value (1 = memory released) is advisory.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn malloc_trim() {}
