use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::terminal_broker::TerminalNoticeKind;
use crate::task::PermissionOp;
use anyhow::Result;
use serde_json::{json, Value};

/// Truncate a UTF-8 string to at most `max_bytes` bytes without splitting a codepoint.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Apply head+tail truncation to `s`, keeping at most `max_bytes` total.
/// If `s` fits within the limit it is returned unchanged. Otherwise the first
/// 4 KB and the last 12 KB are kept with a truncation marker in the middle
/// that reports how many lines were omitted.
fn format_output_head_tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    const HEAD_BYTES: usize = 4 * 1024;
    let tail_bytes = max_bytes.saturating_sub(HEAD_BYTES);
    let head = truncate_utf8(s, HEAD_BYTES);
    let mut tail_start = s.len().saturating_sub(tail_bytes);
    while tail_start < s.len() && !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let omitted = s[head.len()..tail_start].lines().count();
    format!(
        "{}\n[... OUTPUT_TRUNCATED: {} middle lines omitted (kept first 4KB + last 12KB — errors and summaries at the end are preserved). Pipe through grep/head if you need the middle. ...]\n{}",
        head,
        omitted,
        &s[tail_start..]
    )
}

/// Maximum output returned to the model for an inline-completed command.
const OUTPUT_MAX_BYTES: usize = 16 * 1024;

/// Default tail size for `read_terminal_output`.
const READ_OUTPUT_DEFAULT: usize = 8 * 1024;
/// Hard cap for `read_terminal_output`.
const READ_OUTPUT_MAX: usize = 32 * 1024;

/// How long `run_command` waits inline for the command to finish before
/// returning STILL_RUNNING and handing off to the wake-on-completion path.
const RUN_GRACE: std::time::Duration = std::time::Duration::from_secs(25);
/// Poll cadence for the inline completion wait. Sentinel-wrapped commands are
/// detected from their end marker, so a short interval is what makes fast
/// commands return about as fast as a human sees them finish.
const RUN_POLL: std::time::Duration = std::time::Duration::from_millis(25);

pub fn definitions(available_shells: &[String]) -> Vec<ToolDef> {
    let (shell_param, shell_desc) = if available_shells.is_empty() {
        (None, String::new())
    } else {
        let list = available_shells.join(", ");
        let desc = format!(
            "\n\nOptional `shell` selects which shell interprets `command`. Available on this host: {}. Omit to use the platform default. `shell` is ignored when `terminal_id` is set (the existing session's shell is already running).",
            list,
        );
        let schema = json!({
            "type": "string",
            "enum": available_shells,
            "description": format!(
                "Shell to run the command in. Must be one of the values available on this host: {}. Omit to use the platform default. Ignored when terminal_id is set.",
                list,
            ),
        });
        (Some(schema), desc)
    };

    let mut run_command_props = json!({
        "command": { "type": "string", "description": "The command to run" },
        "cwd": { "type": "string", "description": "Working directory relative to the project root (optional; ignored when terminal_id is set — the existing shell keeps its own cwd)" },
        "terminal_id": {
            "type": "integer",
            "description": "OMIT THIS for a normal command — a fresh terminal is spawned automatically. Only pass it to reuse a specific existing session whose id you got from a previous run_command result or list_all_terminals (e.g. a shell with an activated venv), or to type into a USER-opened terminal (always requires explicit user approval). Never guess or default this value."
        },
        "parallel": {
            "type": "boolean",
            "description": "Set true on EACH of several run_command calls emitted in the same response to run them at the same time instead of one after another (e.g. typecheck + lint + tests). Each gets its own fresh terminal. Only for commands that are independent of each other: leave it off when they share a lock, a port or the git index, or when order matters. Ignored on a lone call, when terminal_id is set, or when another kind of tool call sits between them."
        }
    });
    if let (Some(schema), Some(props)) = (shell_param, run_command_props.as_object_mut()) {
        props.insert("shell".into(), schema);
    }

    let run_command_desc = format!(
        "Run a shell command. Every command runs in a pty-backed background terminal; the tool waits up to ~{}s for it to finish. If the command finishes in time, its output is returned directly. If it is still running after the wait (dev servers, watchers, long builds/installs, test suites), you get back a `terminal_id` and the command keeps running without blocking the chat — use `read_terminal_output` to check progress, `kill_terminal` to stop it, or, if you have nothing to do until it finishes, simply end your turn: you are woken automatically with the output once the command completes. Never-ending processes (dev servers, watchers) do not complete — check them with `read_terminal_output` instead of waiting.\n\nFor a normal command, pass only `command` and leave `terminal_id` unset. Pass `terminal_id` ONLY to run a follow-up in the same shell session (e.g. after activating a virtualenv), using an id you got from a previous run_command result or `list_all_terminals`. You may also pass the id of a USER-opened terminal — that always asks the user for approval first.\n\nTo run independent commands at the same time (e.g. typecheck + lint + tests), emit them as separate run_command calls in ONE response, each with `parallel: true`. Leave it off for commands that share a lock, a port or the git index, or that must run in order.{}",
        RUN_GRACE.as_secs(),
        shell_desc,
    );

    vec![
        ToolDef {
            name: "run_command".into(),
            description: run_command_desc,
            parameters: json!({
                "type": "object",
                "properties": run_command_props,
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "read_terminal_output".into(),
            description: "Read recent output from a terminal — one you started via run_command OR a user-opened terminal listed by list_all_terminals. By default returns up to the last ~32KB of buffered scrollback as readable plain text: escape sequences, cursor moves and carriage-return repaints are already resolved, so progress bars collapse to their final frame. Set `rendered: true` to instead get only the *current visible screen* — use this for TUIs (vim, htop, lazygit, anything that redraws in place) where the final screen matters more than the history. Use the default to check progress of a long-running command — e.g. whether a dev server is up, a build finished, or a `pip install` completed.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "terminal_id": {
                        "type": "integer",
                        "description": "Session id returned by run_command or listed by list_all_terminals"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Maximum bytes to return from the tail of the buffer (default 8192, max 32768). Ignored when rendered=true."
                    },
                    "rendered": {
                        "type": "boolean",
                        "description": "When true, return the current visible screen as plain text (escape codes resolved) instead of the raw byte buffer. Best for TUIs and colorized output. Default false."
                    }
                },
                "required": ["terminal_id"]
            }),
        },
        ToolDef {
            name: "kill_terminal".into(),
            description: "Stop and close a terminal. Use this when the process is no longer needed (dev server no longer required, build finished and you want to free the slot). Idempotent — safe to call on an already-closed id. Closing a USER-opened terminal always asks the user for approval first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "terminal_id": {
                        "type": "integer",
                        "description": "Session id returned by run_command or listed by list_all_terminals"
                    }
                },
                "required": ["terminal_id"]
            }),
        },
        ToolDef {
            name: "list_all_terminals".into(),
            description: "List every terminal visible to you: background terminals belonging to THIS task, plus any terminals the USER opened themselves in the integrated terminal panel. Returns one entry per terminal with its `terminal_id`, owner (agent or user), the most recent command sent to it, the working directory, and the label. Use this to find a `terminal_id` for read_terminal_output / run_command / kill_terminal, to check what's still alive before reusing or killing one, or to see what the user has running (e.g. when they mention 'my terminal' without giving an id). You can read any listed terminal freely; running commands in or killing a USER terminal asks the user for approval. Agent terminals from other concurrent tasks are filtered out.".into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
    ]
}

pub async fn execute(
    name: &str,
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    match name {
        "run_command" => run_command(tool_use_id, params, context).await,
        "read_terminal_output" => read_terminal_output(params, context).await,
        "kill_terminal" => kill_terminal(params, context).await,
        "list_all_terminals" => list_all_terminals(context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown terminal tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

/// Shorten a command for one-line display without splitting a codepoint.
fn short_command(cmd: &str) -> &str {
    truncate_utf8(cmd, 200)
}

async fn list_all_terminals(context: &ToolContext) -> Result<ToolOutput> {
    let broker = match context.agent_terminals.as_ref() {
        Some(b) => b,
        None => {
            return Ok(ToolOutput {
                content: "Terminal listing is not available in this environment.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let mut mine = Vec::new();
    let mut user = Vec::new();
    for s in broker.list_all_sessions() {
        if s.is_agent {
            if s.task_id.as_deref() == Some(context.task_id.as_str()) {
                mine.push(s);
            }
        } else {
            user.push(s);
        }
    }

    if mine.is_empty() && user.is_empty() {
        return Ok(ToolOutput {
            content: "No terminals are running — neither background terminals for this task nor user-opened ones.".into(),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let fmt_line = |t: &crate::task::terminal_broker::AgentTerminalInfo| {
        let cmd = t
            .last_command
            .as_deref()
            .map(short_command)
            .unwrap_or("(no command sent yet)");
        format!(
            "- terminal_id={} label=\"{}\" cwd=\"{}\" command=\"{}\"\n",
            t.session_id, t.label, t.cwd, cmd
        )
    };

    let mut body = String::new();
    if !mine.is_empty() {
        body.push_str(&format!("{} terminal(s) owned by this task:\n", mine.len()));
        for t in &mine {
            body.push_str(&fmt_line(t));
        }
    }
    if !user.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&format!(
            "{} USER-opened terminal(s) — you may read them freely; running commands in or killing one asks the user for approval:\n",
            user.len()
        ));
        for t in &user {
            body.push_str(&fmt_line(t));
        }
    }
    body.push_str(
        "\nUse `read_terminal_output(terminal_id)` to read output, `run_command(command, terminal_id)` to run in an existing session, or `kill_terminal(terminal_id)` to stop one.",
    );
    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments: Vec::new(),
    })
}

/// Agent terminals this process spawned with a POSIX shell (zsh/bash/sh…).
/// Only those get the sentinel wrapper — PowerShell, fish and user-opened
/// terminals keep the plain typed-command path.
static POSIX_SESSIONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));
static SENTINEL_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// True when `shell` (a name or a path; `None` = the agent default) is a POSIX shell.
fn is_posix_shell(shell: Option<&str>) -> bool {
    let resolved = shell
        .map(str::to_string)
        .or_else(crate::agent_shell::agent_shell_path);
    let Some(s) = resolved else {
        return !cfg!(target_os = "windows");
    };
    let base = s
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&s)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    matches!(base.as_str(), "zsh" | "bash" | "sh" | "dash" | "ksh")
}

/// Resolve a raw pty buffer into the readable output of `cmd` alone: escape
/// sequences and cursor noise removed, ConPTY's prompt repaints and the command
/// echo cut, trailing prompt dropped.
fn slice_output_since_command(raw: &str, cmd: &str) -> String {
    crate::terminal_text::clean_command_output(raw, cmd)
}

/// A validated, user-approved command ready to be typed into a terminal.
struct PreparedCommand {
    command: String,
    shell: Option<String>,
    cwd: std::path::PathBuf,
    terminal_id: Option<u64>,
    stale_id_note: String,
}

async fn run_command(
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    match prepare_command(&params, context).await {
        Ok(prepared) => launch_command(tool_use_id, prepared, context, true).await,
        Err(out) => Ok(out),
    }
}

/// True when a `run_command` call opted into concurrent execution and targets a fresh terminal.
pub(crate) fn wants_parallel(params: &Value) -> bool {
    let flagged = match &params["parallel"] {
        Value::Bool(b) => *b,
        Value::String(s) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    };
    flagged && params.get("terminal_id").is_none_or(Value::is_null)
}

/// Run several `parallel: true` commands at once, returning outputs in call order.
///
/// Approvals are requested one at a time and a single file-history checkpoint
/// is taken before anything launches: a per-command checkpoint taken while a
/// sibling is already running would fold that sibling's writes into the
/// baseline and drop them from the task's change set.
pub(crate) async fn run_commands_parallel(
    calls: &[(&str, &Value)],
    context: &ToolContext,
) -> Vec<ToolOutput> {
    let mut prepared = Vec::with_capacity(calls.len());
    for (_, params) in calls {
        prepared.push(prepare_command(params, context).await);
    }
    if prepared.iter().any(|p| p.is_ok()) {
        checkpoint_before_command(context);
    }
    let launches = calls
        .iter()
        .zip(prepared)
        .map(|((tool_use_id, _), prep)| async move {
            match prep {
                Ok(p) => launch_command(tool_use_id, p, context, false)
                    .await
                    .unwrap_or_else(|e| ToolOutput {
                        content: format!("Tool error: {}", e),
                        is_error: true,
                        attachments: Vec::new(),
                    }),
                Err(out) => out,
            }
        });
    futures::future::join_all(launches).await
}

/// Fold pending watcher events into the pre-command file-history checkpoint.
fn checkpoint_before_command(context: &ToolContext) {
    if let (Some(history), Some(message_id)) = (
        context.file_history.as_ref(),
        context.current_user_message_id.as_ref(),
    ) {
        let _ = history.checkpoint_pre_bash(message_id);
    }
}

/// Validate a `run_command` call and obtain the user's approval where the mode requires it.
async fn prepare_command(
    params: &Value,
    context: &ToolContext,
) -> std::result::Result<PreparedCommand, ToolOutput> {
    let cmd_str = params["command"].as_str().unwrap_or("");
    if cmd_str.is_empty() {
        return Err(ToolOutput {
            content: "No command provided".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    if context.permissions() == PermissionLevel::Chat {
        return Err(ToolOutput {
            content: "PERMISSION_DENIED: Command execution is not allowed in Chat mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let broker = match context.agent_terminals.as_ref() {
        Some(b) => b,
        None => {
            return Err(ToolOutput {
                content: "Command execution is not available in this environment.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let terminal_id = parse_terminal_id(params);
    let shell = params["shell"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // SECURITY: resolve `cwd` through the same canonical-prefix scope check the
    // file tools use — a bare `project_root.join(cwd)` lets an absolute path or
    // `..` traversal run the command anywhere on disk. The check is waived only
    // when the user turned on "Grant access to all files".
    let cwd = match params["cwd"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(c) => match super::file_ops::resolve_with_scope(context, c) {
            Ok(p) => p,
            Err(out) => return Err(out),
        },
        None => context.project_root.clone(),
    };

    // Classify the target before asking for approval so the preview can say
    // whose terminal the command will run in. A terminal_id pointing at a
    // USER-opened terminal is allowed, but ALWAYS requires explicit approval
    // regardless of the task's permission mode.
    let mut target_user_terminal = false;
    let mut stale_id_note = String::new();
    let mut terminal_id = terminal_id;
    if let Some(id) = terminal_id {
        let info = broker
            .list_all_sessions()
            .into_iter()
            .find(|s| s.session_id == id);
        match info {
            None => {
                stale_id_note = format!(
                    "NOTE: terminal #{} is not an active terminal — the command ran in a freshly spawned terminal instead. Omit terminal_id unless you are reusing a session id from a previous run_command result or list_all_terminals.\n\n",
                    id
                );
                terminal_id = None;
            }
            Some(s) => target_user_terminal = !s.is_agent,
        }
    }

    // SECURITY: pass the full, untruncated command to the permission broker.
    // A previous version truncated at 60 chars, letting prompt-injected commands
    // hide a malicious payload after a benign prefix (e.g. `npm test  # ; curl … | sh`).
    if context.needs_exec_approval() || target_user_terminal {
        let shell_tag = shell
            .as_deref()
            .map(|s| format!(" ({})", s))
            .unwrap_or_default();
        // Surface a non-default working directory in the approval preview so
        // the user sees where the command will actually run.
        let cwd_tag = if terminal_id.is_none() && cwd != context.project_root {
            format!(" [cwd: {}]", cwd.display())
        } else {
            String::new()
        };
        let preview = match terminal_id {
            Some(id) if target_user_terminal => {
                format!("[in YOUR terminal #{}]{} {}", id, shell_tag, cmd_str)
            }
            Some(id) => format!("[in terminal #{}]{} {}", id, shell_tag, cmd_str),
            None => format!("{}{}{}", shell_tag, cmd_str, cwd_tag),
        };
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::RunCommand(preview),
            )
            .await;
        if !approved {
            return Err(ToolOutput {
                content: "PERMISSION_DENIED: User denied command execution.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }

    Ok(PreparedCommand {
        command: cmd_str.to_string(),
        shell,
        cwd,
        terminal_id,
        stale_id_note,
    })
}

/// Type a prepared command into its terminal and wait inline for it to finish.
async fn launch_command(
    tool_use_id: &str,
    prepared: PreparedCommand,
    context: &ToolContext,
    checkpoint: bool,
) -> Result<ToolOutput> {
    let PreparedCommand {
        command,
        shell,
        cwd,
        terminal_id,
        stale_id_note,
    } = prepared;
    let cmd_str = command.as_str();
    let Some(broker) = context.agent_terminals.as_ref() else {
        return Ok(ToolOutput {
            content: "Command execution is not available in this environment.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    };

    // Capture before send: any file the command writes will have mtime >= this
    // instant, so the sweep enqueued on completion picks the changes up.
    let bash_start_for_sweep = std::time::SystemTime::now();
    if checkpoint {
        checkpoint_before_command(context);
    }

    let session_id = match terminal_id {
        Some(id) => id,
        None => {
            match broker.spawn(
                cwd.clone(),
                derive_label(cmd_str),
                &context.task_id,
                shell.clone(),
            ) {
                Ok(id) => {
                    if is_posix_shell(shell.as_deref()) {
                        POSIX_SESSIONS
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(id);
                    }
                    id
                }
                Err(e) => {
                    return Ok(ToolOutput {
                        content: format!("Failed to spawn terminal: {}", e),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            }
        }
    };

    let short_cmd = truncate_utf8(cmd_str, 57);
    context.emit_progress(tool_use_id, &format!("$ [#{session_id}] {short_cmd}"));

    let wrapped = POSIX_SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&session_id);
    let mut end_tag: Option<String> = None;
    let send_result = if wrapped {
        let nonce = SENTINEL_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        end_tag = Some(format!("__RSTC_E_{nonce}_"));
        let line = crate::terminal_text::wrap_posix_command(cmd_str, nonce);
        broker.send_command_line(session_id, cmd_str, &line, &context.task_id)
    } else {
        broker.send_command(session_id, cmd_str, &context.task_id)
    };
    if let Err(e) = send_result {
        return Ok(ToolOutput {
            content: format!("Failed to send command to terminal #{}: {}", session_id, e),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    if let Some(tag) = end_tag.as_deref() {
        broker.expect_end(session_id, tag);
    }

    // Inline wait: poll the targeted notice queue up to RUN_GRACE. The session
    // monitor queues a CommandFinished (or Exited) notice when the shell drops
    // back to its prompt; taking it here consumes it so the same completion is
    // never re-delivered as a wake-up message.
    let deadline = tokio::time::Instant::now() + RUN_GRACE;
    loop {
        tokio::time::sleep(RUN_POLL).await;
        let mut notices = broker.take_command_finished(&context.task_id, session_id);
        // The end marker is printed by the shell itself right after the
        // command returns — a definitive signal, so don't wait for the
        // monitor's process-tree poll + idle confirmation (3–23 s).
        let mut sentinel_done = false;
        if notices.is_empty() {
            if let Some(tag) = end_tag.as_deref() {
                if let Ok(tail) = broker.read_output(session_id, 8 * 1024) {
                    if crate::terminal_text::sanitize_terminal_output(&tail).contains(tag) {
                        broker.finish_command(session_id);
                        notices = broker.take_command_finished(&context.task_id, session_id);
                        sentinel_done = true;
                    }
                }
            }
        }
        if sentinel_done || !notices.is_empty() {
            if let (Some(worker), Some(message_id)) = (
                context.sweep_worker.as_ref(),
                context.current_user_message_id.as_ref(),
            ) {
                let _ = worker.enqueue(crate::file_history::SweepJob {
                    task_id: context.task_id.clone(),
                    message_id: message_id.clone(),
                    bash_start: bash_start_for_sweep,
                });
            }

            let shell_exited = notices.iter().any(|n| n.kind == TerminalNoticeKind::Exited);
            // Prefer a fresh, larger read over the 4KB tail captured at
            // notice time; fall back to the notice tail if the session is
            // already gone.
            let mut raw = broker
                .read_output(session_id, READ_OUTPUT_MAX)
                .unwrap_or_else(|_| {
                    notices
                        .last()
                        .map(|n| n.output_tail.clone())
                        .unwrap_or_default()
                });
            // Completion is detected from the process tree, which can beat the
            // last pty bytes into the buffer — the "(no output)" / truncated
            // results. With a sentinel we know exactly when output is complete.
            if wrapped && !shell_exited {
                for _ in 0..20 {
                    let done = crate::terminal_text::extract_sentinel_output(
                        &crate::terminal_text::sanitize_terminal_output(&raw),
                    )
                    .is_some_and(|s| s.exit_code.is_some());
                    if done {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if let Ok(fresh) = broker.read_output(session_id, READ_OUTPUT_MAX) {
                        raw = fresh;
                    }
                }
            }
            let sliced = slice_output_since_command(&raw, cmd_str);
            let exit_note = crate::terminal_text::extract_sentinel_output(
                &crate::terminal_text::sanitize_terminal_output(&raw),
            )
            .and_then(|s| s.exit_code)
            .filter(|c| *c != 0)
            .map(|c| format!("\n[exit code {c}]"))
            .unwrap_or_default();
            let body = format_output_head_tail(&sliced, OUTPUT_MAX_BYTES);
            let footer = if shell_exited {
                format!(
                    "{exit_note}\n\n[terminal #{} exited — the shell is gone; the next run_command will spawn a fresh one]",
                    session_id
                )
            } else {
                format!(
                    "{exit_note}\n\n[finished in terminal #{id}; the shell is still open — pass terminal_id={id} to run a follow-up in the same session]",
                    id = session_id
                )
            };
            let content = if body.trim().is_empty() {
                format!("{}(no output){}", stale_id_note, footer)
            } else {
                format!("{}{}{}", stale_id_note, body, footer)
            };
            return Ok(ToolOutput {
                content,
                is_error: false,
                attachments: Vec::new(),
            });
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }

    Ok(ToolOutput {
        content: format!(
            "{note}STILL_RUNNING: the command has been running for {}s in terminal #{id} and continues in the background without blocking the chat.\n\
             - read_terminal_output({id}) — check progress\n\
             - kill_terminal({id}) — stop it\n\
             - If you have nothing to do until it finishes, end your turn: you are woken automatically with the output once the command completes. Never-ending processes (dev servers, watchers) do not complete — check them with read_terminal_output instead of waiting.",
            RUN_GRACE.as_secs(),
            id = session_id,
            note = stale_id_note,
        ),
        is_error: false,
        attachments: Vec::new(),
    })
}

fn derive_label(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("agent");
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let short = truncate_utf8(base, 20);
    format!("agent: {}", short)
}

/// Lenient `terminal_id` parser — accepts either a JSON integer or a string
/// that parses as an unsigned integer. We declare `terminal_id` as
/// `type: "integer"` in the tool schema, but some models (the agent in the
/// 2026-05-25 bug report being one of them) call with `{"terminal_id": "3"}`
/// anyway. Failing the call leaves the agent unable to read its own
/// terminals; tolerating both shapes here costs nothing.
fn parse_terminal_id(params: &Value) -> Option<u64> {
    if let Some(n) = params["terminal_id"].as_u64() {
        return Some(n);
    }
    if let Some(s) = params["terminal_id"].as_str() {
        return s.trim().parse::<u64>().ok();
    }
    None
}

async fn read_terminal_output(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let broker = match context.agent_terminals.as_ref() {
        Some(b) => b,
        None => {
            return Ok(ToolOutput {
                content: "Terminal read is not available in this environment.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let id = match parse_terminal_id(&params) {
        Some(v) => v,
        None => {
            return Ok(ToolOutput {
                content: "terminal_id is required".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let max_bytes = params["max_bytes"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(READ_OUTPUT_DEFAULT)
        .min(READ_OUTPUT_MAX);

    // `rendered: true` asks the headless emulator for the current visible
    // screen (escape codes resolved); otherwise we return the raw byte tail.
    let rendered = params["rendered"].as_bool().unwrap_or(false);

    let result = if rendered {
        broker
            .render_screen(id)
            .map(|s| crate::terminal_text::hide_sentinels(&s))
    } else {
        broker.read_output(id, max_bytes).map(|raw| {
            crate::terminal_text::hide_sentinels(&crate::terminal_text::sanitize_terminal_output(
                &raw,
            ))
        })
    };

    match result {
        Ok(text) => {
            let body = if text.trim().is_empty() {
                format!("(terminal #{} has produced no output yet)", id)
            } else {
                text
            };
            Ok(ToolOutput {
                content: body,
                is_error: false,
                attachments: Vec::new(),
            })
        }
        Err(e) => Ok(ToolOutput {
            content: format!("Failed to read terminal #{}: {}", id, e),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn kill_terminal(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let broker = match context.agent_terminals.as_ref() {
        Some(b) => b,
        None => {
            return Ok(ToolOutput {
                content: "Terminal kill is not available in this environment.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let id = match parse_terminal_id(&params) {
        Some(v) => v,
        None => {
            return Ok(ToolOutput {
                content: "terminal_id is required".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    // Closing a USER-opened terminal is intrusive — always ask, regardless of
    // the task's permission mode. Unknown ids fall through to the idempotent
    // kill below.
    let user_owned = broker
        .list_all_sessions()
        .into_iter()
        .find(|s| s.session_id == id)
        .map(|s| (!s.is_agent, s.label));
    if let Some((true, label)) = user_owned {
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::RunCommand(format!("[close YOUR terminal #{} — \"{}\"]", id, label)),
            )
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied closing their terminal.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }

    match broker.kill(id) {
        Ok(()) => Ok(ToolOutput {
            content: format!("Closed terminal #{}.", id),
            is_error: false,
            attachments: Vec::new(),
        }),
        Err(e) => Ok(ToolOutput {
            content: format!("Failed to close terminal #{}: {}", id, e),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_output_head_tail ──────────────────────────────────────────────

    #[test]
    fn short_output_returned_unchanged() {
        let s = "hello world\n";
        assert_eq!(format_output_head_tail(s, 16 * 1024), s);
    }

    #[test]
    fn exact_limit_returned_unchanged() {
        let s = "a".repeat(16 * 1024);
        assert_eq!(format_output_head_tail(&s, 16 * 1024), s);
    }

    #[test]
    fn oversized_output_contains_truncation_marker() {
        let line = "x".repeat(80) + "\n";
        let big: String = line.repeat(300); // ~24 KB
        let result = format_output_head_tail(&big, 16 * 1024);
        assert!(
            result.contains("OUTPUT_TRUNCATED"),
            "expected truncation marker; got prefix: {}",
            &result[..200.min(result.len())]
        );
    }

    #[test]
    fn truncation_keeps_head_and_tail() {
        // Head: 100 lines of "HEAD_LINE\n" (~1 KB, fits in 4 KB head budget).
        // Middle: enough lines to blow the 16 KB cap.
        // Tail: 100 lines of "TAIL_LINE\n".
        let head_part: String = "HEAD_LINE\n".repeat(100);
        let middle_part: String = (0..300).map(|_| "M".repeat(80) + "\n").collect();
        let tail_part: String = "TAIL_LINE\n".repeat(100);
        let full = format!("{}{}{}", head_part, middle_part, tail_part);

        let result = format_output_head_tail(&full, 16 * 1024);
        assert!(result.starts_with("HEAD_LINE"), "head not preserved");
        assert!(result.ends_with("TAIL_LINE\n"), "tail not preserved");
        assert!(
            result.contains("OUTPUT_TRUNCATED"),
            "truncation marker missing"
        );
    }

    #[test]
    fn omitted_line_count_is_nonzero() {
        // Each "middle" line is exactly 10 bytes ("MIDDLE___\n").
        let head_part = "H\n".repeat(10); // 20 bytes
        let middle_part = "MIDDLE___\n".repeat(2000); // ~20 KB
        let tail_part = "T\n".repeat(10); // 20 bytes
        let full = format!("{}{}{}", head_part, middle_part, tail_part);

        let result = format_output_head_tail(&full, 16 * 1024);
        assert!(
            result.contains("lines omitted"),
            "line count not reported: {}",
            &result[..300.min(result.len())]
        );
        assert!(
            !result.contains("0 middle lines omitted"),
            "count should not be zero"
        );
    }

    #[test]
    fn empty_string_returned_unchanged() {
        assert_eq!(format_output_head_tail("", 16 * 1024), "");
    }

    // ── slice_output_since_command ───────────────────────────────────────────

    #[test]
    fn slice_finds_seeded_marker() {
        let raw = "noise\n$ git status\nOn branch main\n";
        let sliced = slice_output_since_command(raw, "git status");
        assert_eq!(sliced, "On branch main");
    }

    #[test]
    fn slice_uses_last_occurrence() {
        let raw = "$ echo hi\nhi\n$ echo hi\nhi again\n";
        let sliced = slice_output_since_command(raw, "echo hi");
        assert_eq!(sliced, "hi again");
    }

    #[test]
    fn slice_strips_color_wrapped_echo_and_prompt() {
        let raw = "PS D:\\p> \x1b[93mgit\x1b[m status\n\x1b[32mOn branch main\x1b[m\nPS D:\\p> ";
        let sliced = slice_output_since_command(raw, "git status");
        assert_eq!(sliced, "On branch main");
    }

    #[test]
    fn slice_returns_full_buffer_when_not_found() {
        let raw = "some unrelated output\n";
        assert_eq!(
            slice_output_since_command(raw, "git status"),
            "some unrelated output"
        );
    }
    // ── truncate_utf8 ────────────────────────────────────────────────────────

    #[test]
    fn truncate_utf8_does_not_split_codepoint() {
        // "é" (U+00E9) is 2 bytes. At max_bytes=1, the function must not
        // split the codepoint — it must produce a valid UTF-8 slice.
        let s = "aéb"; // 'a'(1) + 'é'(2) + 'b'(1) = 4 bytes total
        let result = truncate_utf8(s, 2);
        assert!(
            std::str::from_utf8(result.as_bytes()).is_ok(),
            "result must be valid UTF-8"
        );
        assert!(result.len() <= 2, "result must not exceed max_bytes");
    }

    #[test]
    fn truncate_utf8_returns_full_when_fits() {
        let s = "hello";
        assert_eq!(truncate_utf8(s, 100), s);
    }

    #[test]
    fn truncate_utf8_exact_boundary() {
        let s = "abcde";
        assert_eq!(truncate_utf8(s, 5), "abcde");
        assert_eq!(truncate_utf8(s, 4), "abcd");
    }

    // ── parallel run_command ─────────────────────────────────────────────────

    use crate::task::terminal_broker::{AgentTerminalExit, AgentTerminalInfo, AgentTerminals};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    /// Broker whose commands only "finish" once `release_at` commands are in flight.
    struct FakeTerminals {
        next_id: AtomicU64,
        sent: Mutex<Vec<u64>>,
        release_at: usize,
    }

    impl FakeTerminals {
        fn new(release_at: usize) -> Arc<Self> {
            Arc::new(Self {
                next_id: AtomicU64::new(1),
                sent: Mutex::new(Vec::new()),
                release_at,
            })
        }
    }

    impl AgentTerminals for FakeTerminals {
        fn spawn(
            &self,
            _cwd: std::path::PathBuf,
            _label: String,
            _task_id: &str,
            _shell: Option<String>,
        ) -> std::result::Result<u64, String> {
            Ok(self.next_id.fetch_add(1, Ordering::SeqCst))
        }

        fn send_command(
            &self,
            session_id: u64,
            _command: &str,
            _task_id: &str,
        ) -> std::result::Result<(), String> {
            self.sent.lock().unwrap().push(session_id);
            Ok(())
        }

        fn read_output(
            &self,
            session_id: u64,
            _max_bytes: usize,
        ) -> std::result::Result<String, String> {
            Ok(format!("out-{session_id}\n"))
        }

        fn kill(&self, _session_id: u64) -> std::result::Result<(), String> {
            Ok(())
        }

        fn is_agent_session(&self, _session_id: u64) -> bool {
            true
        }

        fn list_agent_sessions(&self) -> Vec<AgentTerminalInfo> {
            Vec::new()
        }

        fn take_command_finished(&self, _task_id: &str, session_id: u64) -> Vec<AgentTerminalExit> {
            if self.sent.lock().unwrap().len() < self.release_at {
                return Vec::new();
            }
            vec![AgentTerminalExit {
                session_id,
                label: "fake".into(),
                last_command: None,
                output_tail: String::new(),
                exited_at_ms: 0,
                kind: TerminalNoticeKind::CommandFinished,
                output_delivered: false,
            }]
        }
    }

    /// A non-POSIX shell keeps the plain (unwrapped) completion path, which the fake can drive.
    fn fake_call(command: &str, parallel: bool) -> Value {
        json!({ "command": command, "shell": "fish", "parallel": parallel })
    }

    #[test]
    fn wants_parallel_requires_flag_and_fresh_terminal() {
        assert!(wants_parallel(&json!({ "command": "a", "parallel": true })));
        assert!(wants_parallel(
            &json!({ "command": "a", "parallel": "true" })
        ));
        assert!(wants_parallel(
            &json!({ "command": "a", "parallel": true, "terminal_id": null })
        ));
        assert!(!wants_parallel(&json!({ "command": "a" })));
        assert!(!wants_parallel(
            &json!({ "command": "a", "parallel": false })
        ));
        assert!(!wants_parallel(
            &json!({ "command": "a", "parallel": true, "terminal_id": 3 })
        ));
    }

    #[tokio::test]
    async fn parallel_commands_overlap_and_keep_call_order() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, _rx) = ToolContext::new_test(dir.path().to_path_buf());
        // No command can finish until all three are in flight, so this only
        // completes if they were launched together.
        let fake = FakeTerminals::new(3);
        ctx.agent_terminals = Some(fake.clone());

        let params = [
            fake_call("check", true),
            fake_call("lint", true),
            fake_call("test", true),
        ];
        let calls: Vec<(&str, &Value)> = ["a", "b", "c"].into_iter().zip(params.iter()).collect();
        let outputs = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_commands_parallel(&calls, &ctx),
        )
        .await
        .expect("commands ran one after another instead of together");

        assert_eq!(outputs.len(), 3);
        for (i, out) in outputs.iter().enumerate() {
            let id = i + 1;
            assert!(!out.is_error, "call {id} failed: {}", out.content);
            assert!(
                out.content.contains(&format!("out-{id}")),
                "{}",
                out.content
            );
            assert!(
                out.content.contains(&format!("finished in terminal #{id}")),
                "{}",
                out.content
            );
        }
        assert_eq!(*fake.sent.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn parallel_group_reports_a_bad_call_without_blocking_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, _rx) = ToolContext::new_test(dir.path().to_path_buf());
        let fake = FakeTerminals::new(2);
        ctx.agent_terminals = Some(fake.clone());

        let params = [
            fake_call("check", true),
            json!({ "command": "", "parallel": true }),
            fake_call("test", true),
        ];
        let calls: Vec<(&str, &Value)> = ["a", "b", "c"].into_iter().zip(params.iter()).collect();
        let outputs = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_commands_parallel(&calls, &ctx),
        )
        .await
        .expect("valid commands should still launch together");

        assert!(!outputs[0].is_error);
        assert!(outputs[1].is_error);
        assert!(outputs[1].content.contains("No command provided"));
        assert!(!outputs[2].is_error);
        assert_eq!(fake.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn single_command_still_runs_through_the_staged_path() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, _rx) = ToolContext::new_test(dir.path().to_path_buf());
        ctx.agent_terminals = Some(FakeTerminals::new(1));

        let out = run_command("a", fake_call("check", false), &ctx)
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("out-1"), "{}", out.content);
    }
}
