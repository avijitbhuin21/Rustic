use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
use anyhow::Result;
use serde_json::{json, Value};
use std::process::Command;

/// Spawn the command without flashing a console window on Windows. GUI Tauri
/// processes don't own a console, so child cmd/powershell spawns briefly pop
/// one open by default. CREATE_NO_WINDOW (0x0800_0000) suppresses that.
#[cfg(windows)]
fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000)
}
#[cfg(not(windows))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

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

/// Maximum output returned to the model for a foreground command.
const FG_OUTPUT_MAX_BYTES: usize = 16 * 1024;

/// Default tail size for `read_terminal_output`.
const READ_OUTPUT_DEFAULT: usize = 8 * 1024;
/// Hard cap for `read_terminal_output`.
const READ_OUTPUT_MAX: usize = 32 * 1024;

pub fn definitions(available_shells: &[String]) -> Vec<ToolDef> {
    // Build the `shell` parameter schema + description fragment. When the
    // host can confirm a set of shells, constrain the schema with an `enum`
    // so the model can't ask for something that won't spawn. When no list
    // is available (unit tests, embedded contexts), omit the param entirely
    // and let the platform default take over.
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
        "cwd": { "type": "string", "description": "Working directory relative to the project root (optional)" },
        "background": {
            "type": "boolean",
            "description": "false = wait for completion and return output. true = run persistently in a pty terminal and return a terminal_id without blocking."
        },
        "terminal_id": {
            "type": "integer",
            "description": "Reuse an existing background terminal (e.g. one with an active venv or a REPL). Only valid when background=true."
        }
    });
    if let Some(schema) = shell_param {
        // Safe unwrap: we just built run_command_props as an object literal.
        run_command_props
            .as_object_mut()
            .unwrap()
            .insert("shell".into(), schema);
    }

    let run_command_desc = format!(
        "Run a shell command. Set `background: false` for commands that complete quickly and return output (builds, tests, git, file ops) — the output is returned to you directly. Set `background: true` for long-running or persistent processes (dev servers, watchers, `npm run dev`, `cargo run` of a server, anything that does not exit on its own) — the command runs in a persistent pty-backed terminal without blocking the chat, and you get back a `terminal_id`. \n\nTo reuse a previous background terminal (e.g. to run more commands inside an activated virtualenv or a shell session you already started), pass its `terminal_id`. Omit `terminal_id` to spawn a fresh terminal. After starting a background command, use `read_terminal_output` to check progress and `kill_terminal` when done.{}",
        shell_desc,
    );

    vec![
        ToolDef {
            name: "run_command".into(),
            description: run_command_desc,
            parameters: json!({
                "type": "object",
                "properties": run_command_props,
                "required": ["command", "background"]
            }),
        },
        ToolDef {
            name: "read_terminal_output".into(),
            description: "Read recent output from a background terminal (started via run_command with background=true). Returns up to the last ~32KB of buffered output. Use this to check progress of a long-running command — e.g. to see if a dev server is up, a build finished, or a `pip install` completed.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "terminal_id": {
                        "type": "integer",
                        "description": "Session id returned by run_command(background=true)"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Maximum bytes to return from the tail of the buffer (default 8192, max 32768)"
                    }
                },
                "required": ["terminal_id"]
            }),
        },
        ToolDef {
            name: "kill_terminal".into(),
            description: "Stop and close a background terminal. Use this when the process is no longer needed (dev server no longer required, build finished and you want to free the slot). Idempotent — safe to call on an already-closed id.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "terminal_id": {
                        "type": "integer",
                        "description": "Session id returned by run_command(background=true)"
                    }
                },
                "required": ["terminal_id"]
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
        _ => Ok(ToolOutput {
            content: format!("Unknown terminal tool: {}", name),
            is_error: true,
        }),
    }
}

async fn run_command(
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let cmd_str = params["command"].as_str().unwrap_or("");
    if cmd_str.is_empty() {
        return Ok(ToolOutput {
            content: "No command provided".into(),
            is_error: true,
        });
    }

    let background = params["background"].as_bool().unwrap_or(false);
    let terminal_id = params["terminal_id"].as_u64();
    let shell = params["shell"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Chat mode: hard deny
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Command execution is not allowed in Chat mode.".into(),
            is_error: true,
        });
    }

    // ManualEdit / AutoEdit: ask the user.
    //
    // SECURITY: The full, untruncated command string is passed to the
    // permission broker so the approval UI can render it in its entirety.
    // A previous version truncated this preview at 60 characters, which let
    // prompt-injected commands hide a malicious payload after a benign
    // prefix (e.g. `npm test  # ; curl … | sh`). The UI label that's
    // displayed *during* execution is still allowed to truncate (see
    // emit_progress below) — that's cosmetic, not a security gate.
    if context.needs_exec_approval() {
        let shell_tag = shell
            .as_deref()
            .map(|s| format!(" ({})", s))
            .unwrap_or_default();
        let preview = if background {
            if let Some(id) = terminal_id {
                format!("[background in terminal #{}]{} {}", id, shell_tag, cmd_str)
            } else {
                format!("[background, new terminal]{} {}", shell_tag, cmd_str)
            }
        } else {
            format!("{}{}", shell_tag, cmd_str)
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
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied command execution.".into(),
                is_error: true,
            });
        }
    }

    let cwd = params["cwd"]
        .as_str()
        .map(|c| context.project_root.join(c))
        .unwrap_or_else(|| context.project_root.clone());

    // P0.7: soft-warn when the agent uses a shell read command for a file
    // range (R.2 F2). The result is still returned — we don't reject — but
    // we prepend a one-line nudge toward `read_file` so the agent learns to
    // pick the right tool next time. Detection happens before execution so
    // the warn is reliably attached to *this* invocation's output.
    let shell_read_warn = detect_shell_file_read(cmd_str);

    if background {
        return run_background(tool_use_id, cmd_str, cwd, terminal_id, shell, context);
    }

    let mut output = run_foreground(tool_use_id, cmd_str, cwd, shell.as_deref(), context)?;
    if let Some(warn) = shell_read_warn {
        output.content = format!("{}\n\n{}", warn, output.content);
    }
    Ok(output)
}

/// Detect shell invocations that read a file (or a line range of one) when
/// `read_file` with `start_line`/`end_line` would have been the right tool.
/// Returns the nudge text to prepend to the tool result, or None when the
/// command looks like a legitimate non-read use (build, test, pipeline, etc.).
///
/// Conservative on purpose — false positives waste tokens and annoy the
/// agent. We only fire when the FIRST token (after leading whitespace) is one
/// of the well-known file-read programs and the command shape is a plain
/// "read a file" invocation (no `|`, no `>`, no `&&` that would imply
/// pipelining the read into something else).
fn detect_shell_file_read(cmd: &str) -> Option<&'static str> {
    const WARN: &str = "Note: this command looks like a file read via the shell. \
                        `read_file` with `start_line`/`end_line` is strictly preferred — \
                        it's faster, more reliable on Windows, doesn't burn shell context, \
                        and avoids quoting / line-counting failures. Reach for `read_file` \
                        next time.";

    let trimmed = cmd.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    // Skip if the command is part of a pipeline / compound — those are
    // usually doing something with the read output (filter, count, transform)
    // and the user probably knows what they're doing.
    if trimmed.contains('|') || trimmed.contains('>') || trimmed.contains("&&") || trimmed.contains(";") {
        return None;
    }
    // Tokenize the leading program name (handles `cmd /c findstr` style by
    // looking at both the first and the third tokens).
    let mut tokens = trimmed.split_whitespace();
    let first = tokens.next()?;
    let first_lower = first.to_ascii_lowercase();
    // Strip a Windows .exe suffix and any path prefix so `C:\windows\sed.exe`
    // is recognized the same as `sed`.
    let prog = first_lower
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(&first_lower);
    let prog = prog.strip_suffix(".exe").unwrap_or(prog);

    match prog {
        "cat" | "head" | "tail" | "type" | "sed" => Some(WARN),
        "get-content" | "gc" => Some(WARN),
        // `cmd /c <inner>` — peek at the next-but-one token.
        "cmd" => {
            let _flag = tokens.next();
            let inner = tokens.next().unwrap_or("").to_ascii_lowercase();
            let inner = inner.strip_suffix(".exe").unwrap_or(&inner);
            matches!(inner, "type" | "cat" | "head" | "tail" | "sed").then_some(WARN)
        }
        // `powershell -Command "Get-Content ..."` — look for the read
        // command inside the quoted arg. Cheap substring check is fine; the
        // pipeline guard above already excluded compound forms.
        "powershell" | "pwsh" => {
            let rest = trimmed.to_ascii_lowercase();
            (rest.contains("get-content") || rest.contains(" gc ")).then_some(WARN)
        }
        _ => None,
    }
}

/// Build a (program, args) pair for invoking `cmd_str` through the requested
/// shell. A full path to the shell is also accepted; the base name (minus
/// any `.exe` suffix) picks the argument style.
fn build_shell_invocation(shell: Option<&str>, cmd_str: &str) -> (String, Vec<String>) {
    let Some(raw) = shell else {
        return if cfg!(target_os = "windows") {
            ("cmd".into(), vec!["/C".into(), cmd_str.into()])
        } else {
            ("sh".into(), vec!["-c".into(), cmd_str.into()])
        };
    };
    let base = raw
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    match base {
        "cmd" => (raw.to_string(), vec!["/C".into(), cmd_str.into()]),
        "powershell" | "pwsh" | "ps" => (
            raw.to_string(),
            vec!["-NoProfile".into(), "-Command".into(), cmd_str.into()],
        ),
        // POSIX-style shells all accept `-c "cmd"` (bash, zsh, sh, fish, dash, ash…)
        _ => (raw.to_string(), vec!["-c".into(), cmd_str.into()]),
    }
}

fn run_foreground(
    tool_use_id: &str,
    cmd_str: &str,
    cwd: std::path::PathBuf,
    shell: Option<&str>,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let short_cmd = if cmd_str.len() > 60 { &cmd_str[..57] } else { cmd_str };
    let shell_tag = shell.map(|s| format!(" [{}]", s)).unwrap_or_default();
    context.emit_progress(tool_use_id, &format!("${} {short_cmd}", shell_tag));

    // Capture bash_start BEFORE spawning the child. Any file the command
    // touches will have an mtime >= this instant. If we captured after spawn,
    // a fast command could write its file and return before our SystemTime::now
    // call completed, leaving the mtime just barely under the cutoff.
    let bash_start_for_sweep = std::time::SystemTime::now();

    let pty_session: Option<(u64, std::sync::Arc<dyn crate::AgentTerminals>)> =
        if let Some(broker) = context.agent_terminals.as_ref() {
            let label = format!("$ {short_cmd}");
            match broker.spawn(
                cwd.clone(),
                label,
                &context.task_id,
                None,
            ) {
                Ok(id) => {
                    let prompt = format!("$ {cmd_str}\r\n");
                    let _ = broker.write_raw(id, &prompt);
                    Some((id, std::sync::Arc::clone(broker)))
                }
                Err(_) => None,
            }
        } else {
            None
        };

    let (program, args) = build_shell_invocation(shell, cmd_str);
    let mut cmd = Command::new(&program);
    cmd.args(&args).current_dir(&cwd);
    no_window(&mut cmd);
    let output = tokio::task::block_in_place(|| cmd.output());

    let tool_output = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push_str("\n--- stderr ---\n");
                }
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result = format!(
                    "Command completed with exit code {}",
                    out.status.code().unwrap_or(-1)
                );
            }

            let result = if result.len() > FG_OUTPUT_MAX_BYTES {
                let truncated = truncate_utf8(&result, FG_OUTPUT_MAX_BYTES);
                let remaining_lines = result[truncated.len()..].lines().count();
                format!(
                    "{}\nOUTPUT_TRUNCATED: Truncated at 16KB — {} more lines. Use head/tail/grep to filter.",
                    truncated, remaining_lines
                )
            } else {
                result
            };

            ToolOutput {
                content: result,
                is_error: !out.status.success(),
            }
        }
        Err(e) => ToolOutput {
            content: format!(
                "Failed to execute command via `{}`: {}. If the shell isn't installed, pass a different `shell` value or omit it to use the platform default.",
                program, e
            ),
            is_error: true,
        },
    };

    if let Some((session_id, broker)) = pty_session {
        let display = if tool_output.is_error {
            format!("{}\r\n\x1b[31m[exit: error]\x1b[0m\r\n", tool_output.content)
        } else {
            format!("{}\r\n\x1b[32m[done]\x1b[0m\r\n", tool_output.content)
        };
        let _ = broker.write_raw(session_id, &display);
        let _ = broker.kill(session_id);
    }

    // Enqueue a sweep job for the changed-files tracker. Fire-and-forget —
    // the agent's tool result returns immediately while the worker walks the
    // worktree in the background. See `file_history::sweep` for coalescing
    // semantics when multiple bashes finish near each other.
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

    Ok(tool_output)
}

fn run_background(
    tool_use_id: &str,
    cmd_str: &str,
    cwd: std::path::PathBuf,
    terminal_id: Option<u64>,
    shell: Option<String>,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let broker = match context.agent_terminals.as_ref() {
        Some(b) => b,
        None => {
            return Ok(ToolOutput {
                content: "Background execution is not available in this environment.".into(),
                is_error: true,
            });
        }
    };

    // Resolve target terminal: reuse or spawn new.
    let (session_id, created_new) = match terminal_id {
        Some(id) => {
            if !broker.is_agent_session(id) {
                return Ok(ToolOutput {
                    content: format!(
                        "Terminal #{} is not an active agent terminal. Omit terminal_id to spawn a new one.",
                        id
                    ),
                    is_error: true,
                });
            }
            (id, false)
        }
        None => {
            let label = derive_label(cmd_str);
            match broker.spawn(cwd.clone(), label, &context.task_id, shell.clone()) {
                Ok(id) => (id, true),
                Err(e) => {
                    return Ok(ToolOutput {
                        content: format!("Failed to spawn background terminal: {}", e),
                        is_error: true,
                    });
                }
            }
        }
    };

    let short_cmd = if cmd_str.len() > 60 { &cmd_str[..57] } else { cmd_str };
    context.emit_progress(
        tool_use_id,
        &format!("$ [bg#{session_id}] {short_cmd}"),
    );

    if let Err(e) = broker.send_command(session_id, cmd_str) {
        return Ok(ToolOutput {
            content: format!("Started terminal #{}, but failed to send command: {}", session_id, e),
            is_error: true,
        });
    }

    let prefix = if created_new {
        format!("Spawned new background terminal #{}.", session_id)
    } else {
        format!("Sent command to background terminal #{}.", session_id)
    };
    Ok(ToolOutput {
        content: format!(
            "{} Command is running without blocking the chat. Use read_terminal_output({}) to check progress, kill_terminal({}) to stop. Reuse this terminal_id for follow-up commands that need the same shell session (e.g. after activating a venv).",
            prefix, session_id, session_id
        ),
        is_error: false,
    })
}

/// Take the first token of a command and wrap it as a terminal label.
fn derive_label(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("agent");
    // Strip leading path components (e.g. ./scripts/foo → foo)
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let short = if base.len() > 20 { &base[..20] } else { base };
    format!("agent: {}", short)
}

async fn read_terminal_output(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let broker = match context.agent_terminals.as_ref() {
        Some(b) => b,
        None => {
            return Ok(ToolOutput {
                content: "Terminal read is not available in this environment.".into(),
                is_error: true,
            });
        }
    };

    let id = match params["terminal_id"].as_u64() {
        Some(v) => v,
        None => {
            return Ok(ToolOutput {
                content: "terminal_id is required".into(),
                is_error: true,
            });
        }
    };

    let max_bytes = params["max_bytes"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(READ_OUTPUT_DEFAULT)
        .min(READ_OUTPUT_MAX);

    match broker.read_output(id, max_bytes) {
        Ok(text) => {
            let body = if text.is_empty() {
                format!("(terminal #{} has produced no output yet)", id)
            } else {
                text
            };
            Ok(ToolOutput {
                content: body,
                is_error: false,
            })
        }
        Err(e) => Ok(ToolOutput {
            content: format!("Failed to read terminal #{}: {}", id, e),
            is_error: true,
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
            });
        }
    };

    let id = match params["terminal_id"].as_u64() {
        Some(v) => v,
        None => {
            return Ok(ToolOutput {
                content: "terminal_id is required".into(),
                is_error: true,
            });
        }
    };

    match broker.kill(id) {
        Ok(()) => Ok(ToolOutput {
            content: format!("Closed terminal #{}.", id),
            is_error: false,
        }),
        Err(e) => Ok(ToolOutput {
            content: format!("Failed to close terminal #{}: {}", id, e),
            is_error: true,
        }),
    }
}

#[cfg(test)]
mod p0_7_shell_read_detector {
    use super::detect_shell_file_read;

    #[test]
    fn detects_unix_read_tools() {
        assert!(detect_shell_file_read("cat src/main.rs").is_some());
        assert!(detect_shell_file_read("head -n 50 README.md").is_some());
        assert!(detect_shell_file_read("tail -50 log.txt").is_some());
        assert!(detect_shell_file_read("sed -n '10,40p' file.rs").is_some());
    }

    #[test]
    fn detects_windows_read_tools() {
        assert!(detect_shell_file_read("Get-Content notes.md").is_some());
        assert!(detect_shell_file_read("get-content -Tail 20 server.log").is_some());
        assert!(detect_shell_file_read("type config.json").is_some());
        // PowerShell -Command "Get-Content ..." should be flagged.
        assert!(
            detect_shell_file_read("powershell -Command \"Get-Content config.json\"").is_some()
        );
        // cmd /c type ... should be flagged.
        assert!(detect_shell_file_read("cmd /c type file.txt").is_some());
    }

    #[test]
    fn ignores_pipelines_and_compound_commands() {
        // The agent piping head into something else is doing legit work.
        assert!(detect_shell_file_read("head -50 file.txt | grep TODO").is_none());
        // Redirects almost always mean "do something with the content", not "just read".
        assert!(detect_shell_file_read("cat file.txt > out.txt").is_none());
        // && / ; chains imply a compound flow.
        assert!(detect_shell_file_read("cat a.txt && echo done").is_none());
        assert!(detect_shell_file_read("cat a.txt; cat b.txt").is_none());
    }

    #[test]
    fn ignores_non_read_commands() {
        assert!(detect_shell_file_read("cargo build").is_none());
        assert!(detect_shell_file_read("git status").is_none());
        assert!(detect_shell_file_read("npm test").is_none());
        assert!(detect_shell_file_read("rm tempfile.txt").is_none());
        // `tail -f` is streaming, not a one-shot read — but it's also caught
        // by our detector since the first token is `tail`. That's OK: the
        // soft-warn is informational, not a hard reject, and `tail -f` is
        // an outlier the agent shouldn't be using in normal flow anyway.
    }

    #[test]
    fn handles_path_prefixes_and_exe_suffix() {
        assert!(detect_shell_file_read("C:\\Windows\\System32\\type.exe x.txt").is_some());
        assert!(detect_shell_file_read("/usr/bin/cat /etc/hosts").is_some());
    }

    #[test]
    fn empty_or_whitespace_returns_none() {
        assert!(detect_shell_file_read("").is_none());
        assert!(detect_shell_file_read("   ").is_none());
    }
}
