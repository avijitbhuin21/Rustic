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
    /// True when the shell process has exited but the row is retained so the
    /// user can still read its scrollback. Retired sessions accept no input.
    pub exited: bool,
}

/// How many exited (retired) sessions to retain for the UI before evicting
/// the oldest. Keeps a long-running app from accumulating dead tabs' buffers.
const MAX_EXITED_SESSIONS: usize = 12;
/// Cap on the retained scrollback snapshot per exited session (bytes).
const MAX_EXITED_SCROLLBACK_BYTES: usize = 256 * 1024;

/// Lightweight snapshot of a session whose shell exited: the UI keeps showing
/// the tab (flagged `exited`) with its frozen scrollback until the user
/// explicitly closes it.
struct ExitedSession {
    info: SessionInfo,
    scrollback_ansi: String,
    exited_at_ms: u64,
}

pub struct TerminalManager {
    sessions: HashMap<SessionId, PtySession>,
    exited: HashMap<SessionId, ExitedSession>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            exited: HashMap::new(),
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
        self.exited.remove(&id);
    }

    pub fn exists(&self, id: SessionId) -> bool {
        self.sessions.contains_key(&id)
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(session_info)
            .chain(self.exited.values().map(|e| e.info.clone()))
            .collect()
    }

    /// Read the tail of a session's output buffer as a lossy UTF-8 string.
    pub fn read_output_tail(&self, id: SessionId, max_bytes: usize) -> Result<String> {
        if let Some(session) = self.sessions.get(&id) {
            return Ok(read_tail(&session.output_buffer, max_bytes));
        }
        if let Some(retired) = self.exited.get(&id) {
            let s = &retired.scrollback_ansi;
            let start = s.len().saturating_sub(max_bytes);
            let start = (start..s.len())
                .find(|i| s.is_char_boundary(*i))
                .unwrap_or(s.len());
            return Ok(s[start..].to_string());
        }
        Err(anyhow::anyhow!("Session not found: {}", id))
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

    /// Serialize a session's full scrollback + screen as a clean ANSI string
    /// (see [`TerminalEmulator::render_scrollback_ansi`]). Used by the frontend
    /// to rehydrate an xterm instance's history WITHOUT the duplicated repaint
    /// frames that replaying the raw ConPTY byte buffer produces.
    ///
    /// [`TerminalEmulator::render_scrollback_ansi`]: crate::emulator::TerminalEmulator::render_scrollback_ansi
    pub fn render_scrollback_ansi(&self, id: SessionId) -> Result<String> {
        if let Some(retired) = self.exited.get(&id) {
            return Ok(retired.scrollback_ansi.clone());
        }
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        let emu = session
            .emulator
            .lock()
            .map_err(|_| anyhow::anyhow!("emulator lock poisoned"))?;
        Ok(emu.render_scrollback_ansi())
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

    /// Mark that an agent-issued command was just sent to this session and is
    /// presumed running until the monitor observes the shell back at its prompt.
    pub fn mark_command_in_flight(&self, id: SessionId) -> Result<()> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        if let Ok(mut slot) = session.command_in_flight.lock() {
            *slot = Some(std::time::Instant::now());
        }
        Ok(())
    }

    /// Clear the in-flight command marker (the monitor detected completion).
    pub fn clear_command_in_flight(&self, id: SessionId) {
        if let Some(session) = self.sessions.get(&id) {
            if let Ok(mut slot) = session.command_in_flight.lock() {
                *slot = None;
            }
        }
    }

    /// When an agent command is in flight on this session, returns the instant
    /// it was sent. `None` if idle, unknown, or not an agent session.
    pub fn command_in_flight_since(&self, id: SessionId) -> Option<std::time::Instant> {
        self.sessions
            .get(&id)
            .and_then(|s| s.command_in_flight.lock().ok().and_then(|g| *g))
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
    ) -> Option<(Option<String>, String, Option<String>, String, bool)> {
        let session = self.sessions.remove(&id)?;
        let task_id = session.task_id.lock().ok().and_then(|g| g.clone());
        let label = session.label.clone();
        let last_command = session.last_command.lock().ok().and_then(|g| g.clone());
        let tail = crate::pty::read_tail(&session.output_buffer, tail_bytes);
        let command_was_in_flight = session
            .command_in_flight
            .lock()
            .ok()
            .map(|g| g.is_some())
            .unwrap_or(false);
        // Retire (rather than forget) user-owned sessions so the UI can keep
        // the tab visible with an "exited" marker + frozen scrollback. Agent
        // sessions route their exit to the owning task instead and vanish.
        if !session.is_agent {
            let mut scrollback_ansi = session
                .emulator
                .lock()
                .ok()
                .map(|emu| emu.render_scrollback_ansi())
                .unwrap_or_default();
            if scrollback_ansi.len() > MAX_EXITED_SCROLLBACK_BYTES {
                let start = scrollback_ansi.len() - MAX_EXITED_SCROLLBACK_BYTES;
                let start = (start..scrollback_ansi.len())
                    .find(|i| scrollback_ansi.is_char_boundary(*i))
                    .unwrap_or(0);
                scrollback_ansi = scrollback_ansi[start..].to_string();
            }
            let mut info = session_info(&session);
            info.exited = true;
            let exited_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            self.exited.insert(
                id,
                ExitedSession {
                    info,
                    scrollback_ansi,
                    exited_at_ms,
                },
            );
            while self.exited.len() > MAX_EXITED_SESSIONS {
                let oldest = self
                    .exited
                    .iter()
                    .min_by_key(|(_, e)| e.exited_at_ms)
                    .map(|(k, _)| *k);
                match oldest {
                    Some(k) => {
                        self.exited.remove(&k);
                    }
                    None => break,
                }
            }
        }
        Some((task_id, label, last_command, tail, command_was_in_flight))
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
        exited: false,
    }
}
