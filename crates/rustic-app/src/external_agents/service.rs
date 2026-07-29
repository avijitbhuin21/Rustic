//! Host-agnostic orchestration for external CLI agent sessions.
//!
//! Both hosts (the Tauri desktop shell and `rustic-server`) drive external
//! agents through this module, so the spawn recipe — hook config, correlation
//! token, database row, command line — lives in exactly one place. What the
//! hosts keep is only the part that is genuinely theirs: creating the PTY and
//! wiring its output stream.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use rustic_db::{Database, ExternalAgentSessionRow};

use super::antigravity;
use super::hooks::HookPaths;
use super::spool::drain_spool;
use super::{build_launch_plan, AgentKind, AgentPermissionMode, LaunchMode, ShellKind};
use crate::context::EventEmitter;
use crate::sync_ext::MutexExt;

/// Event telling the frontend to re-read a project's external agent sessions.
pub const SESSIONS_CHANGED_EVENT: &str = "external-agent-sessions-changed";

/// How often the spool is folded into the database.
const DRAIN_INTERVAL: Duration = Duration::from_millis(1500);
/// How long to keep watching for an `agy` conversation after spawning one.
const AGY_CAPTURE_WINDOW: Duration = Duration::from_secs(600);
const AGY_POLL_INTERVAL: Duration = Duration::from_millis(750);

static DRAIN_THREAD: OnceLock<()> = OnceLock::new();

/// What the caller wants to start.
#[derive(Debug, Clone)]
pub enum SpawnTarget {
    /// A brand-new conversation, optionally kicked off with a first prompt.
    New {
        agent: AgentKind,
        project_id: String,
        prompt: Option<String>,
        /// Autonomy level to launch the CLI with.
        permission: AgentPermissionMode,
    },
    /// Continue an existing conversation, identified by our own row id.
    Resume {
        session_row_id: String,
        /// Autonomy level to launch the CLI with.
        permission: AgentPermissionMode,
    },
}

/// Everything the host needs to open the PTY, once the shared side is done.
#[derive(Debug, Clone)]
pub struct SpawnPrep {
    /// `external_agent_sessions.id` of the row backing this tab.
    pub row_id: String,
    pub agent: AgentKind,
    /// Correlation token exported as `RUSTIC_PTY_ID`.
    pub pty_key: String,
    /// Tab label.
    pub label: String,
    /// Working directory for the PTY (the project root).
    pub cwd: PathBuf,
    /// Command line to write into the shell, without a trailing newline.
    pub command_line: String,
    /// Extra PTY environment.
    pub env: Vec<(String, String)>,
    /// Conversation ids present before spawning, for Antigravity's
    /// difference-based capture.
    pub agy_before: Option<HashSet<String>>,
}

/// Prepare an external agent launch: write hook config, register (or rebind) the
/// database row, and build the command line.
///
/// Does not touch the terminal — the host calls this, opens a PTY with
/// [`SpawnPrep::env`], writes [`SpawnPrep::command_line`] into it, then calls
/// [`start_capture`].
pub fn prepare(
    db: &Database,
    app_data_dir: &Path,
    project_root_of: impl Fn(&str) -> Result<PathBuf>,
    target: SpawnTarget,
    shell: ShellKind,
) -> Result<SpawnPrep> {
    let paths = HookPaths::ensure(app_data_dir)?;
    let pty_key = uuid::Uuid::new_v4().simple().to_string();

    let (agent, project_id, row_id, cwd, mode, permission, existing) = match target {
        SpawnTarget::New {
            agent,
            project_id,
            prompt,
            permission,
        } => {
            let cwd = project_root_of(&project_id)?;
            let row_id = uuid::Uuid::new_v4().to_string();
            (
                agent,
                project_id,
                row_id,
                cwd,
                LaunchMode::New { prompt },
                permission,
                None,
            )
        }
        SpawnTarget::Resume {
            session_row_id,
            permission,
        } => {
            let row = db
                .get_external_agent_session(&session_row_id)?
                .ok_or_else(|| anyhow!("session {session_row_id} not found"))?;
            let agent = AgentKind::from_str(&row.agent)
                .ok_or_else(|| anyhow!("unknown agent `{}`", row.agent))?;
            let external = row.external_session_id.clone().ok_or_else(|| {
                anyhow!("this session has no id yet — it can't be resumed until its first turn")
            })?;
            let cwd = project_root_of(&row.project_id)?;
            (
                agent,
                row.project_id.clone(),
                row.id.clone(),
                cwd,
                LaunchMode::Resume {
                    external_session_id: external,
                },
                permission,
                Some(row),
            )
        }
    };

    // Hook registration is per-agent: Claude takes a settings file, Codex needs
    // a file inside the project, Antigravity supports neither and is captured by
    // watching its conversation store instead.
    let mut agy_before = None;
    match agent {
        AgentKind::Claude => super::hooks::write_claude_settings(&paths)?,
        AgentKind::Codex => super::hooks::write_codex_hooks(&cwd, &paths)?,
        AgentKind::Antigravity => {
            agy_before = antigravity::conversations_dir().map(|d| antigravity::snapshot_ids(&d));
        }
    }

    let claude_settings =
        matches!(agent, AgentKind::Claude).then_some(paths.claude_settings.as_path());
    // Resolved per spawn, not cached across spawns: if the user updated the CLI
    // from inside a previous session, this is where the new copy gets picked up.
    let resolved = super::launcher::resolve(agent.program());
    if let Some(found) = resolved.as_ref() {
        tracing::debug!(
            target: "rustic::external_agents",
            agent = agent.as_str(),
            path = %found.path.display(),
            version = found.version.as_deref().unwrap_or("unknown"),
            "resolved launcher"
        );
    }
    let plan = build_launch_plan(
        agent,
        &mode,
        permission,
        shell,
        &pty_key,
        &paths.spool,
        claude_settings,
        resolved.as_ref().map(|r| r.path.as_path()),
    );

    if existing.is_some() {
        db.rebind_external_agent_session(&row_id, &pty_key)?;
    } else {
        db.create_external_agent_session(
            &row_id,
            &project_id,
            agent.as_str(),
            &pty_key,
            &cwd.to_string_lossy(),
        )?;
        // Claude lets Rustic dictate the session id, so the row is resumable
        // straight away instead of only after the first hook fires.
        if let Some(sid) = plan.preassigned_session_id.as_deref() {
            db.attach_external_agent_session_id(&pty_key, sid, None)?;
        }
    }

    let label = existing
        .as_ref()
        .and_then(|r| r.title.clone())
        .unwrap_or_else(|| agent.label().to_string());

    Ok(SpawnPrep {
        row_id,
        agent,
        pty_key,
        label,
        cwd,
        command_line: plan.command_line,
        env: plan.env,
        agy_before,
    })
}

/// Start the background capture for a spawned session: the shared spool drain
/// loop, plus Antigravity's per-spawn conversation watch.
pub fn start_capture(
    db: Arc<Mutex<Database>>,
    emitter: Arc<dyn EventEmitter>,
    app_data_dir: &Path,
    prep: &SpawnPrep,
) {
    let Ok(paths) = HookPaths::ensure(app_data_dir) else {
        return;
    };
    ensure_drain_thread(db.clone(), emitter.clone(), paths.spool.clone());

    if let (AgentKind::Antigravity, Some(before)) = (prep.agent, prep.agy_before.clone()) {
        let pty_key = prep.pty_key.clone();
        std::thread::spawn(move || watch_antigravity(db, emitter, pty_key, before));
    }
}

/// Watch for the conversation an `agy` spawn creates and fold it in.
///
/// Two stages, because Antigravity writes the conversation file when it opens
/// but only records the prompt once the user submits a turn: the id is attached
/// as soon as the file appears (which makes the session resumable), then polling
/// continues until a title shows up or the window closes.
///
/// The database lock is taken per update and never across a sleep — this loop
/// can run for ten minutes.
fn watch_antigravity(
    db: Arc<Mutex<Database>>,
    emitter: Arc<dyn EventEmitter>,
    pty_key: String,
    before: HashSet<String>,
) {
    let Some(dir) = antigravity::conversations_dir() else {
        return;
    };
    let started = std::time::Instant::now();
    let mut found: Option<antigravity::Conversation> = None;

    while started.elapsed() < AGY_CAPTURE_WINDOW {
        std::thread::sleep(AGY_POLL_INTERVAL);

        match found.as_ref() {
            None => {
                let Some(conversation) = antigravity::find_new_conversation(&dir, &before) else {
                    continue;
                };
                let attached = {
                    let db = db.lock_safe();
                    match db.attach_external_agent_session_id(&pty_key, &conversation.id, None) {
                        // The row is gone (tab closed and deleted) — stop watching.
                        Ok(None) => return,
                        Err(e) => {
                            tracing::warn!(target: "rustic::external_agents", "agy: attach failed: {e}");
                            return;
                        }
                        Ok(Some(_)) => {
                            if let Some(title) = conversation.title.as_deref() {
                                let _ = db.set_external_agent_session_title(&pty_key, title);
                            }
                            true
                        }
                    }
                };
                if attached {
                    emitter.emit_json(SESSIONS_CHANGED_EVENT, serde_json::Value::Null);
                }
                if conversation.title.is_some() {
                    return;
                }
                found = Some(conversation);
            }
            Some(conversation) => {
                let Some(title) =
                    antigravity::read_conversation(&conversation.path).and_then(|c| c.title)
                else {
                    continue;
                };
                let _ = db
                    .lock_safe()
                    .set_external_agent_session_title(&pty_key, &title);
                emitter.emit_json(SESSIONS_CHANGED_EVENT, serde_json::Value::Null);
                return;
            }
        }
    }
}

/// Launch the process-wide spool drain loop, once.
///
/// One long-lived loop (rather than one per spawn) means a hook that fires long
/// after its spawn — or while Rustic was restarting — is still folded in on the
/// next pass.
fn ensure_drain_thread(db: Arc<Mutex<Database>>, emitter: Arc<dyn EventEmitter>, spool: PathBuf) {
    if DRAIN_THREAD.set(()).is_err() {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(DRAIN_INTERVAL);
        let outcome = {
            let db = db.lock_safe();
            drain_spool(&db, &spool)
        };
        if outcome.changed() {
            emitter.emit_json(SESSIONS_CHANGED_EVENT, serde_json::Value::Null);
        }
    });
}

/// Sessions for a project, newest first.
pub fn list_sessions(
    db: &Database,
    project_id: &str,
    agent: Option<AgentKind>,
    limit: i64,
) -> Result<Vec<ExternalAgentSessionRow>> {
    Ok(db.list_external_agent_sessions(project_id, agent.map(|a| a.as_str()), limit)?)
}

/// Delete a conversation. With `purge_transcript` the CLI's own record goes too,
/// so the session also stops showing up in that tool's native resume picker.
///
/// Purging is best effort: a transcript the CLI has already moved or rotated is
/// not worth failing the delete over.
pub fn delete_session(db: &Database, session_row_id: &str, purge_transcript: bool) -> Result<()> {
    if purge_transcript {
        if let Some(row) = db.get_external_agent_session(session_row_id)? {
            purge_transcript_files(&row);
        }
    }
    db.delete_external_agent_session(session_row_id)?;
    Ok(())
}

fn purge_transcript_files(row: &ExternalAgentSessionRow) {
    for path in transcript_targets(row) {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(
                target: "rustic::external_agents",
                path = %path.display(),
                "purged CLI transcript"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                target: "rustic::external_agents",
                path = %path.display(),
                "purging CLI transcript failed: {e}"
            ),
        }
    }
}

/// Files holding the CLI's own copy of a conversation.
fn transcript_targets(row: &ExternalAgentSessionRow) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = Vec::new();
    // Claude and Codex report their transcript path through the hooks.
    if let Some(path) = row.transcript_path.as_deref() {
        targets.push(PathBuf::from(path));
    }
    // Antigravity has no hooks: its conversations are one SQLite file per id in
    // the store we watch, so the path is derived rather than reported.
    if let (Some(AgentKind::Antigravity), Some(id)) = (
        AgentKind::from_str(&row.agent),
        row.external_session_id.as_deref(),
    ) {
        if let Some(dir) = antigravity::conversations_dir() {
            let derived = dir.join(format!("{id}.db"));
            if !targets.contains(&derived) {
                targets.push(derived);
            }
        }
    }

    let mut all: Vec<PathBuf> = Vec::new();
    for path in targets {
        // SQLite leaves -wal/-shm next to the database it journals.
        if path.extension().is_some_and(|e| e == "db") {
            for suffix in ["-wal", "-shm"] {
                let mut sidecar = path.clone().into_os_string();
                sidecar.push(suffix);
                all.push(PathBuf::from(sidecar));
            }
        }
        all.push(path);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        agent: &str,
        transcript: Option<&str>,
        external: Option<&str>,
    ) -> ExternalAgentSessionRow {
        ExternalAgentSessionRow {
            id: "row".into(),
            project_id: "proj".into(),
            agent: agent.into(),
            pty_key: "key".into(),
            external_session_id: external.map(String::from),
            title: None,
            transcript_path: transcript.map(String::from),
            cwd: "/tmp".into(),
            created_at: String::new(),
            updated_at: String::new(),
            last_active_at: None,
        }
    }

    #[test]
    fn targets_the_hook_reported_transcript() {
        let targets = transcript_targets(&row("claude", Some("/tmp/session.jsonl"), Some("abc")));
        assert_eq!(targets, vec![PathBuf::from("/tmp/session.jsonl")]);
    }

    #[test]
    fn targets_nothing_when_no_transcript_is_known() {
        assert!(transcript_targets(&row("codex", None, None)).is_empty());
    }

    #[test]
    fn targets_sqlite_sidecars_alongside_the_database() {
        let targets = transcript_targets(&row("antigravity", Some("/tmp/conv.db"), None));
        assert_eq!(
            targets,
            vec![
                PathBuf::from("/tmp/conv.db-wal"),
                PathBuf::from("/tmp/conv.db-shm"),
                PathBuf::from("/tmp/conv.db"),
            ]
        );
    }
}
