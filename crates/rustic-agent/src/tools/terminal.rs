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
        ToolDef {
            name: "list_all_terminals".into(),
            description: "List every background terminal currently running for THIS task. Returns one entry per terminal with its `terminal_id`, the most recent command sent to it, the working directory, and the label. Use this when you've lost track of which terminals you spawned, want to check what's still alive before reusing or killing one, or need a `terminal_id` to pass to `read_terminal_output` / `kill_terminal`. Terminals from other concurrent tasks are filtered out — you only see your own.".into(),
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

    let mine: Vec<_> = broker
        .list_agent_sessions()
        .into_iter()
        .filter(|s| s.task_id.as_deref() == Some(context.task_id.as_str()))
        .collect();

    if mine.is_empty() {
        return Ok(ToolOutput {
            content: "No background terminals are running for this task.".into(),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let mut body = format!("{} terminal(s) running for this task:\n", mine.len());
    for t in &mine {
        let cmd = t
            .last_command
            .as_deref()
            .map(|c| if c.len() > 200 { &c[..200] } else { c })
            .unwrap_or("(no command sent yet)");
        body.push_str(&format!(
            "- terminal_id={} label=\"{}\" cwd=\"{}\" command=\"{}\"\n",
            t.session_id, t.label, t.cwd, cmd
        ));
    }
    body.push_str(
        "\nUse `read_terminal_output(terminal_id)` to read output, or `kill_terminal(terminal_id)` to stop one.",
    );
    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments: Vec::new(),
    })
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
            is_error: true, attachments: Vec::new() });
    }

    let background = params["background"].as_bool().unwrap_or(false);
    let terminal_id = params["terminal_id"].as_u64();
    let shell = params["shell"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Command execution is not allowed in Chat mode.".into(),
            is_error: true, attachments: Vec::new() });
    }

    // SECURITY: pass the full, untruncated command to the permission broker.
    // A previous version truncated at 60 chars, letting prompt-injected commands
    // hide a malicious payload after a benign prefix (e.g. `npm test  # ; curl … | sh`).
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
                is_error: true, attachments: Vec::new() });
        }
    }

    let cwd = params["cwd"]
        .as_str()
        .map(|c| context.project_root.join(c))
        .unwrap_or_else(|| context.project_root.clone());

    if let Some(err) = detect_shell_file_read(cmd_str) {
        return Ok(crate::tools::ToolOutput::text(err, true));
    }

    if background {
        return run_background(tool_use_id, cmd_str, cwd, terminal_id, shell, context);
    }

    let output = run_foreground(tool_use_id, cmd_str, cwd, shell.as_deref(), context)?;
    Ok(output)
}

const SHELL_READ_BLOCKED: &str =
    "SHELL_READ_BLOCKED: Shell commands cannot be used to read file contents.\n\
     Use `read_file` with `offset` / `limit` instead — it is faster, works \
     correctly on Windows, and does not burn shell context:\n\
     \n\
     read_file(path=\"<file>\", offset=<start_line>, limit=<line_count>)\n\
     \n\
     Do NOT retry this command. Switch to `read_file` now.";

/// Returns `Some(SHELL_READ_BLOCKED)` when the command is a shell-based file read.
/// Detection is intentionally broad — pipeline/semicolon forms are blocked too, because
/// the agent exploited `|`/`;` bailouts (e.g. `Get-Content f; $lines[N..M]`) to bypass.
fn detect_shell_file_read(cmd: &str) -> Option<&'static str> {
    if cmd.trim().is_empty() {
        return None;
    }

    let lower = cmd.to_ascii_lowercase();

    if lower.contains("get-content") {
        return Some(SHELL_READ_BLOCKED);
    }

    // Block `findstr /N` (the line-numbering trick); plain findstr without /N is a grep-style search.
    if lower.contains("findstr") {
        let has_n_flag = lower
            .split_whitespace()
            .any(|tok| {
                (tok.starts_with('/') || tok.starts_with('-'))
                    && tok[1..].to_ascii_lowercase().contains('n')
            });
        if has_n_flag {
            return Some(SHELL_READ_BLOCKED);
        }
    }

    let first_token = cmd.trim_start().split_whitespace().next().unwrap_or("");
    let prog_lower = first_token.to_ascii_lowercase();
    let prog = prog_lower
        .rsplit(|c: char| c == '/' || c == '\\')
        .next()
        .unwrap_or(&prog_lower);
    let prog = prog.strip_suffix(".exe").unwrap_or(prog);

    match prog {
        "cat" | "head" | "tail" | "type" | "sed" | "get-content" | "gc" => {
            Some(SHELL_READ_BLOCKED)
        }
        "cmd" => {
            let mut tokens = cmd.trim_start().split_whitespace().skip(1);
            let _flag = tokens.next(); // /c or /C
            let inner = tokens.next().unwrap_or("").to_ascii_lowercase();
            let inner = inner.strip_suffix(".exe").unwrap_or(&inner);
            matches!(inner, "type" | "cat" | "head" | "tail" | "sed")
                .then_some(SHELL_READ_BLOCKED)
        }
        // `get-content` is caught above; also block the `gc` alias inside -Command strings.
        "powershell" | "pwsh" => {
            let has_gc_alias = lower
                .split_whitespace()
                .any(|t| t == "gc" || t.starts_with("gc'") || t.starts_with("gc\""));
            has_gc_alias.then_some(SHELL_READ_BLOCKED)
        }
        _ => None,
    }
}

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

    // Capture before spawn: any file the command writes will have mtime >= this instant.
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
                is_error: !out.status.success(), attachments: Vec::new() }
        }
        Err(e) => ToolOutput {
            content: format!(
                "Failed to execute command via `{}`: {}. If the shell isn't installed, pass a different `shell` value or omit it to use the platform default.",
                program, e
            ),
            is_error: true,
            attachments: Vec::new(),
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
                is_error: true, attachments: Vec::new() });
        }
    };

    let (session_id, created_new) = match terminal_id {
        Some(id) => {
            if !broker.is_agent_session(id) {
                return Ok(ToolOutput {
                    content: format!(
                        "Terminal #{} is not an active agent terminal. Omit terminal_id to spawn a new one.",
                        id
                    ),
                    is_error: true,
                    attachments: Vec::new(),
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
                        attachments: Vec::new(),
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
            attachments: Vec::new(),
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
        attachments: Vec::new(),
    })
}

fn derive_label(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("agent");
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
                is_error: true, attachments: Vec::new() });
        }
    };

    let id = match params["terminal_id"].as_u64() {
        Some(v) => v,
        None => {
            return Ok(ToolOutput {
                content: "terminal_id is required".into(),
                is_error: true, attachments: Vec::new() });
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
                is_error: false, attachments: Vec::new() })
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
                is_error: true, attachments: Vec::new() });
        }
    };

    let id = match params["terminal_id"].as_u64() {
        Some(v) => v,
        None => {
            return Ok(ToolOutput {
                content: "terminal_id is required".into(),
                is_error: true, attachments: Vec::new() });
        }
    };

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
        assert!(detect_shell_file_read("powershell -Command \"Get-Content config.json\"").is_some());
        assert!(detect_shell_file_read("cmd /c type file.txt").is_some());
    }

    #[test]
    fn blocks_pipeline_and_compound_forms() {
        assert!(detect_shell_file_read("head -50 file.txt | grep TODO").is_some());
        assert!(detect_shell_file_read("cat file.txt > out.txt").is_some());
        assert!(detect_shell_file_read("cat a.txt && echo done").is_some());
        assert!(detect_shell_file_read("cat a.txt; cat b.txt").is_some());
        assert!(detect_shell_file_read(
            "$lines = Get-Content 'crates/rustic-db/src/file_history_repo.rs'; $lines[200..250]"
        ).is_some());
        assert!(detect_shell_file_read(
            "Get-Content crates/rustic-agent/src/tools/subagent_tools.rs | Select-Object -Skip 290 -First 130"
        ).is_some());
        assert!(detect_shell_file_read(
            "powershell -NoProfile -Command \"(Get-Content 'crates/rustic-db/src/file_history_repo.rs')[200..250]\""
        ).is_some());
    }

    #[test]
    fn blocks_findstr_n_line_numbering_trick() {
        assert!(detect_shell_file_read(
            "$lines = findstr /N \".\" crates\\rustic-db\\src\\file_history_repo.rs"
        ).is_some());
        assert!(detect_shell_file_read("findstr /N \".\" file.rs").is_some());
        assert!(detect_shell_file_read("findstr /n \".\" file.rs").is_some());
        assert!(detect_shell_file_read("findstr \"fn open_snapshot\" crates/rustic-db/src/file_history_repo.rs").is_none());
    }

    #[test]
    fn ignores_non_read_commands() {
        assert!(detect_shell_file_read("cargo build").is_none());
        assert!(detect_shell_file_read("git status").is_none());
        assert!(detect_shell_file_read("npm test").is_none());
        assert!(detect_shell_file_read("rm tempfile.txt").is_none());
        assert!(detect_shell_file_read("cargo check -p rustic-db 2>&1 | tail -20").is_none());
        assert!(detect_shell_file_read("git diff --stat").is_none());
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
