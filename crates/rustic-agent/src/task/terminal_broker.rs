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
    /// Task id that spawned this terminal. `None` for terminals not tagged to a task.
    pub task_id: Option<String>,
    pub created_at_ms: u64,
}

/// What kind of background-terminal event a notice describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalNoticeKind {
    /// The agent-issued command finished; the shell is still alive at its
    /// prompt and the terminal id remains usable.
    CommandFinished,
    /// The shell process itself exited (crash, `exit`, idle reclaim) and the
    /// terminal is gone.
    Exited,
}

/// Record of a background terminal event (command completed or shell exited).
/// Populated by the host app and drained by the executor loop (mid-turn) or
/// the host's auto-resume path (idle task) to synthesize a user-visible
/// notification message for the model.
#[derive(Debug, Clone)]
pub struct AgentTerminalExit {
    pub session_id: u64,
    pub label: String,
    pub last_command: Option<String>,
    /// Tail of the session's output buffer at event time (UTF-8 lossy).
    pub output_tail: String,
    /// Unix-ms timestamp when the event was observed.
    pub exited_at_ms: u64,
    pub kind: TerminalNoticeKind,
}

/// Render queued terminal notices as one synthetic SYSTEM user-message body.
/// Shared by the executor's mid-turn drain and the hosts' idle auto-resume so
/// both paths produce the identical message shape.
pub fn format_terminal_notices(notices: &[AgentTerminalExit]) -> String {
    let mut body = String::from(
        "SYSTEM: background terminal update — one or more background commands you \
         started have finished or their terminal has exited. Review the output \
         below and decide whether to verify results, restart, fix a bug, or \
         proceed.\n",
    );
    for n in notices {
        match n.kind {
            TerminalNoticeKind::CommandFinished => {
                body.push_str(&format!(
                    "\n— Terminal #{} ({}): command finished; the terminal is still \
                     open (reuse it with run_command or read more output with \
                     read_terminal_output).",
                    n.session_id, n.label
                ));
            }
            TerminalNoticeKind::Exited => {
                body.push_str(&format!(
                    "\n— Terminal #{} ({}): the shell process exited — the terminal \
                     is no longer running.",
                    n.session_id, n.label
                ));
            }
        }
        if let Some(cmd) = &n.last_command {
            body.push_str(&format!("\nLast command: {}", cmd));
        }
        body.push_str("\nOutput (tail):\n```\n");
        if n.output_tail.trim().is_empty() {
            body.push_str("(no output)\n");
        } else {
            body.push_str(n.output_tail.trim_end());
            body.push('\n');
        }
        body.push_str("```\n");
    }
    body
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

    /// Render the terminal's *current visible screen* as plain text, with all
    /// escape sequences resolved by a headless emulator — i.e. what a human
    /// would see on screen right now, instead of the raw byte scrollback that
    /// `read_output` returns. Ideal for TUIs (vim, htop, lazygit) and any
    /// colorized output. Default impl falls back to `read_output` for brokers
    /// that don't maintain an emulator.
    fn render_screen(&self, session_id: u64) -> Result<String, String> {
        self.read_output(session_id, 8 * 1024)
    }

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

    /// Write raw bytes directly into a session's output ring-buffer and emit
    /// them as a `terminal-output` event so the xterm instance updates live.
    /// Used by foreground commands to display captured output without running
    /// a second shell process inside the PTY.
    fn write_raw(&self, session_id: u64, data: &str) -> Result<(), String>;
}
