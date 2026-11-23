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
    /// True for terminals spawned by an agent; false for terminals the user
    /// opened themselves in the integrated terminal panel.
    pub is_agent: bool,
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
    /// True when no command was in flight at event time, i.e. the last
    /// command's output already reached the task (inline `run_command` result
    /// or an earlier CommandFinished notice) and must not be replayed.
    pub output_delivered: bool,
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
    // A terminal reclaimed between its command finishing and this batch being
    // delivered has both a CommandFinished and an Exited notice. Report it
    // once, as closed: saying "still open" and then "exited" sends the model
    // to a terminal that is gone.
    let finished: std::collections::HashSet<u64> = notices
        .iter()
        .filter(|n| n.kind == TerminalNoticeKind::CommandFinished)
        .map(|n| n.session_id)
        .collect();
    let exited: std::collections::HashSet<u64> = notices
        .iter()
        .filter(|n| n.kind == TerminalNoticeKind::Exited)
        .map(|n| n.session_id)
        .collect();
    for n in notices {
        if n.kind == TerminalNoticeKind::Exited && n.output_delivered {
            if finished.contains(&n.session_id) {
                continue;
            }
            body.push_str(&format!(
                "\n— Terminal #{} ({}): the shell process exited — the terminal is no \
                 longer running. Its last command had already finished and its output \
                 was returned to you earlier, so it is not repeated here.\n",
                n.session_id, n.label
            ));
            continue;
        }
        match n.kind {
            TerminalNoticeKind::CommandFinished if exited.contains(&n.session_id) => {
                body.push_str(&format!(
                    "\n— Terminal #{} ({}): command finished, and the terminal has \
                     since closed. Its last output is below and stays readable with \
                     read_terminal_output; a new run_command starts a fresh terminal.",
                    n.session_id, n.label
                ));
            }
            TerminalNoticeKind::CommandFinished => {
                body.push_str(&format!(
                    "\n— Terminal #{} ({}): command finished; the terminal is still \
                     open for now (reuse it with run_command or read more output with \
                     read_terminal_output). An idle agent terminal closes on its own \
                     shortly after; its last output stays readable.",
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
        let tail = match &n.last_command {
            Some(cmd) => crate::terminal_text::clean_command_output(&n.output_tail, cmd),
            None => crate::terminal_text::strip_trailing_prompt(
                &crate::terminal_text::sanitize_terminal_output(&n.output_tail),
            ),
        };
        if tail.trim().is_empty() {
            body.push_str("(no output)\n");
        } else {
            body.push_str(tail.trim_end());
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
    /// Also records the command on the session for UI display, marks a
    /// command-in-flight so the session monitor can detect completion, and
    /// tags the session with `task_id` so completion/exit notices route back
    /// to the task that issued the command (this is how a USER-opened
    /// terminal gets adopted for notice routing when the agent runs a
    /// command inside it).
    fn send_command(&self, session_id: u64, command: &str, task_id: &str) -> Result<(), String>;

    /// Like `send_command`, but types `line` into the pty while recording
    /// `display` as the command shown in the UI and in completion notices.
    /// Used for sentinel-wrapped commands; brokers that don't override it
    /// just type `line`.
    fn send_command_line(
        &self,
        session_id: u64,
        display: &str,
        line: &str,
        task_id: &str,
    ) -> Result<(), String> {
        let _ = display;
        self.send_command(session_id, line, task_id)
    }

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

    /// List EVERY live session — agent-owned and user-opened alike. Used by
    /// the terminal tools to give the agent visibility into terminals the
    /// user spawned. Default impl falls back to agent-only for brokers that
    /// don't distinguish.
    fn list_all_sessions(&self) -> Vec<AgentTerminalInfo> {
        self.list_agent_sessions()
    }

    /// Remove and return any queued notices for `session_id` from `task_id`'s
    /// pending-exit queue, leaving other sessions' notices in place. The
    /// `run_command` tool polls this while it waits inline for a command to
    /// finish, so a completion it consumed is never re-delivered as a
    /// synthetic wake-up message. Default impl returns empty.
    fn take_command_finished(&self, _task_id: &str, _session_id: u64) -> Vec<AgentTerminalExit> {
        Vec::new()
    }

    /// Tell the host which end marker the just-sent sentinel-wrapped command
    /// prints, so its session monitor can wake the task the moment it appears
    /// instead of waiting on process-tree polling. Default impl is a no-op.
    fn expect_end(&self, _session_id: u64, _marker: &str) {}

    /// `run_command` saw the command's end marker and already returned its
    /// output: clear the in-flight marker so the session monitor does not
    /// queue a second (duplicate) completion notice. Default impl is a no-op.
    fn finish_command(&self, _session_id: u64) {}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(kind: TerminalNoticeKind, output_delivered: bool) -> AgentTerminalExit {
        AgentTerminalExit {
            session_id: 7,
            label: "agent: git".into(),
            last_command: Some("git status".into()),
            output_tail: "$ git status\nnothing to commit\n".into(),
            exited_at_ms: 0,
            kind,
            output_delivered,
        }
    }

    #[test]
    fn idle_shell_exit_does_not_replay_delivered_output() {
        let body = format_terminal_notices(&[notice(TerminalNoticeKind::Exited, true)]);
        assert!(
            body.contains("Terminal #7 (agent: git): the shell process exited"),
            "{body}"
        );
        assert!(!body.contains("nothing to commit"), "{body}");
        assert!(!body.contains("Last command"), "{body}");
        assert!(!body.contains("Output (tail)"), "{body}");
    }

    /// Tracker issue #14: a terminal that closed before its finish notice was
    /// delivered is reported once, as closed — never as "still open".
    #[test]
    fn finished_then_closed_terminal_is_reported_once_as_closed() {
        let body = format_terminal_notices(&[
            notice(TerminalNoticeKind::CommandFinished, false),
            notice(TerminalNoticeKind::Exited, true),
        ]);
        assert_eq!(body.matches("— Terminal #7").count(), 1, "{body}");
        assert!(body.contains("has since closed"), "{body}");
        assert!(!body.contains("still open"), "{body}");
        assert!(!body.contains("shell process exited"), "{body}");
        assert!(
            body.contains("Output (tail):\n```\nnothing to commit\n```"),
            "{body}"
        );
    }

    #[test]
    fn finished_notice_for_another_terminal_keeps_its_own_exit_notice() {
        let mut other = notice(TerminalNoticeKind::Exited, true);
        other.session_id = 8;
        let body =
            format_terminal_notices(&[notice(TerminalNoticeKind::CommandFinished, false), other]);
        assert!(
            body.contains(
                "Terminal #7 (agent: git): command finished; the terminal is still open for now"
            ),
            "{body}"
        );
        assert!(
            body.contains("Terminal #8 (agent: git): the shell process exited"),
            "{body}"
        );
    }

    #[test]
    fn exit_mid_command_and_command_finished_keep_their_output() {
        for kind in [
            TerminalNoticeKind::Exited,
            TerminalNoticeKind::CommandFinished,
        ] {
            let body = format_terminal_notices(&[notice(kind, false)]);
            assert!(body.contains("Last command: git status"), "{body}");
            assert!(
                body.contains("Output (tail):\n```\nnothing to commit\n```"),
                "{body}"
            );
        }
    }
}
