use crate::emulator::TerminalEmulator;
use crate::pty::{append_output, read_tail, BoxedChild, PtySession, SessionId};
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub label: String,
    pub cwd: String,
    pub is_agent: bool,
    /// OS process id of the spawned shell. `None` if the backend couldn't
    /// obtain one (rare; some pty implementations don't expose it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Most recent command sent by the agent to this terminal, if any. Used for UI display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_command: Option<String>,
    /// Task id that spawned this session, if it was spawned by an agent task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Unix-ms timestamp when the session was created.
    pub created_at_ms: u64,
}

pub struct TerminalManager {
    sessions: HashMap<SessionId, PtySession>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Create a new terminal session. Returns the session info, the reader
    /// to stream output, and a handle to the shared output buffer (so the
    /// caller can fan output into both the Tauri event stream and the buffer).
    #[allow(clippy::type_complexity)]
    pub fn create_session(
        &mut self,
        cwd: PathBuf,
        label: String,
        is_agent: bool,
        shell_program: Option<String>,
        initial_size: Option<(u16, u16)>,
    ) -> Result<(
        SessionInfo,
        Box<dyn std::io::Read + Send>,
        Arc<Mutex<VecDeque<u8>>>,
        Arc<Mutex<TerminalEmulator>>,
        BoxedChild,
    )> {
        let mut session = PtySession::new(cwd, label, is_agent, shell_program, initial_size)?;
        let reader = session
            .take_reader()
            .ok_or_else(|| anyhow::anyhow!("Reader already taken"))?;
        let child = session
            .take_child()
            .ok_or_else(|| anyhow::anyhow!("Child already taken"))?;
        let buffer = Arc::clone(&session.output_buffer);
        let emulator = Arc::clone(&session.emulator);

        let info = session_info(&session);
        self.sessions.insert(session.id, session);
        Ok((info, reader, buffer, emulator, child))
    }

    pub fn write_session(&mut self, id: SessionId, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        session.write(data)
    }

    pub fn resize_session(&self, id: SessionId, cols: u16, rows: u16) -> Result<()> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        session.resize(cols, rows)
    }

    pub fn destroy_session(&mut self, id: SessionId) {
        self.sessions.remove(&id);
    }

    pub fn exists(&self, id: SessionId) -> bool {
        self.sessions.contains_key(&id)
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.values().map(session_info).collect()
    }

    /// Read the tail of a session's output buffer as a lossy UTF-8 string.
    pub fn read_output_tail(&self, id: SessionId, max_bytes: usize) -> Result<String> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        Ok(read_tail(&session.output_buffer, max_bytes))
    }

    /// Render the *current visible screen* of a session as plain text, with all
    /// escape sequences resolved by the headless emulator. This is what the
    /// agent should read when it wants "what's on screen now" (e.g. a TUI),
    /// versus `read_output_tail` which returns the raw byte scrollback.
    pub fn render_screen(&self, id: SessionId) -> Result<String> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        let emu = session
            .emulator
            .lock()
            .map_err(|_| anyhow::anyhow!("emulator lock poisoned"))?;
        Ok(emu.render_screen())
    }

    /// Record the most recent agent-issued command on a session (for UI display).
    pub fn set_last_command(&self, id: SessionId, command: &str) -> Result<()> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        if let Ok(mut slot) = session.last_command.lock() {
            *slot = Some(command.to_string());
        }
        Ok(())
    }

    /// Append a literal string to the session's output buffer. Used to record
    /// agent-issued commands so they appear in `read_terminal_output` alongside
    /// actual pty output (pty echo may lag on some platforms).
    pub fn append_buffer(&self, id: SessionId, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        append_output(&session.output_buffer, data);
        Ok(())
    }

    pub fn is_agent(&self, id: SessionId) -> bool {
        self.sessions.get(&id).map(|s| s.is_agent).unwrap_or(false)
    }

    /// Tag an (agent-owned) session with the task_id that spawned it, so the
    /// output-reader thread can route pty-exit notifications back to that task.
    pub fn set_task_id(&self, id: SessionId, task_id: &str) -> Result<()> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        if let Ok(mut slot) = session.task_id.lock() {
            *slot = Some(task_id.to_string());
        }
        Ok(())
    }

    /// Atomically remove a session AND return the data needed for its exit
    /// notification. Because both the output-reader thread (on EOF) and the
    /// session-monitor thread (on shell exit / idle timeout) race to finalize
    /// the same session, this is the single gate that decides which one "wins":
    /// the `HashMap::remove` is the atomic operation, so exactly one caller
    /// gets `Some(..)` and proceeds to notify/emit; the loser gets `None` and
    /// does nothing. Returns `None` for an already-removed (or never-existing)
    /// session.
    pub fn take_for_exit(
        &mut self,
        id: SessionId,
        tail_bytes: usize,
    ) -> Option<(Option<String>, String, Option<String>, String)> {
        let session = self.sessions.remove(&id)?;
        let task_id = session.task_id.lock().ok().and_then(|g| g.clone());
        let label = session.label.clone();
        let last_command = session.last_command.lock().ok().and_then(|g| g.clone());
        let tail = crate::pty::read_tail(&session.output_buffer, tail_bytes);
        Some((task_id, label, last_command, tail))
    }
}

fn session_info(s: &PtySession) -> SessionInfo {
    SessionInfo {
        id: s.id,
        label: s.label.clone(),
        cwd: s.cwd.to_string_lossy().to_string(),
        is_agent: s.is_agent,
        pid: s.pid,
        last_command: s.last_command.lock().ok().and_then(|g| g.clone()),
        task_id: s.task_id.lock().ok().and_then(|g| g.clone()),
        created_at_ms: s.created_at_ms,
    }
}
