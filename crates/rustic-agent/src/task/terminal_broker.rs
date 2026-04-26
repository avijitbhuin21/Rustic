//! Broker trait for agent-owned terminal sessions.
//!
//! The rustic-agent crate does not depend on rustic-terminal or Tauri; instead
//! the host app (src-tauri) provides an implementation of `AgentTerminals` that
//! wires the agent's `run_command`/`read_terminal_output`/`kill_terminal` tools
//! into the shared pty-backed TerminalManager.
//!
//! Keeping this as a trait lets the agent tool code stay testable and keeps the
//! rustic-agent crate free of a transitive pty/Tauri dependency.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AgentTerminalInfo {
    pub session_id: u64,
    pub label: String,
    pub cwd: String,
    pub last_command: Option<String>,
    pub created_at_ms: u64,
}

/// Record of a background terminal that ended (shell process exited).
/// Populated by the host app when a pty reader hits EOF and drained by the
/// executor loop to synthesize a user-visible notification message for the
/// model.
#[derive(Debug, Clone)]
pub struct AgentTerminalExit {
    pub session_id: u64,
    pub label: String,
    pub last_command: Option<String>,
    /// Tail of the session's output buffer at exit time (UTF-8 lossy).
    pub output_tail: String,
    /// Unix-ms timestamp when the exit was observed.
    pub exited_at_ms: u64,
}

pub trait AgentTerminals: Send + Sync {
    /// Spawn a new pty-backed terminal flagged as agent-owned and tagged with
    /// `task_id` so its eventual exit is routed back to the right task.
    /// Returns the new session id.
    ///
    /// `shell` is an optional override: a short name (`bash`, `pwsh`,
    /// `powershell`, `cmd`, `zsh`, `sh`, `fish`, …) or a full path to a shell
    /// executable. When `None`, the broker picks a platform-appropriate
    /// default.
    fn spawn(
        &self,
        cwd: PathBuf,
        label: String,
        task_id: &str,
        shell: Option<String>,
    ) -> Result<u64, String>;

    /// Write a command line (followed by a newline) to an existing terminal.
    /// Also records the command on the session for UI display.
    fn send_command(&self, session_id: u64, command: &str) -> Result<(), String>;

    /// Read the tail of a terminal's buffered output as a UTF-8 string.
    fn read_output(&self, session_id: u64, max_bytes: usize) -> Result<String, String>;

    /// Close a terminal (kills the underlying shell). Idempotent on unknown ids.
    fn kill(&self, session_id: u64) -> Result<(), String>;

    /// Returns true if a session exists and is agent-owned.
    fn is_agent_session(&self, session_id: u64) -> bool;

    /// List agent-owned sessions (filtered to is_agent=true).
    fn list_agent_sessions(&self) -> Vec<AgentTerminalInfo>;

    /// Drain the pending-exit queue for `task_id`. Called by the executor at
    /// the top of each loop iteration so background-terminal crashes become
    /// synthetic user messages the model sees on the next provider call.
    /// Default impl returns empty for brokers that don't track exits.
    fn drain_pending_exits(&self, _task_id: &str) -> Vec<AgentTerminalExit> {
        Vec::new()
    }

    /// Short names of shells confirmed to exist on this host (e.g. `["cmd",
    /// "pwsh", "bash"]`). Used by `run_command` to narrow the tool schema so
    /// the model can only ask for shells that will actually spawn. Default
    /// impl returns empty — the tool then omits the `shell` parameter
    /// entirely and falls back to the platform default.
    fn available_shells(&self) -> Vec<String> {
        Vec::new()
    }
}
