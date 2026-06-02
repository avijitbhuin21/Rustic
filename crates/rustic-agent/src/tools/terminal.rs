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
            "description": "Optional (default false). false = wait for completion and return output — use ONLY for commands that finish in well under 30s (git, file ops, quick builds/tests). true = run persistently in a pty terminal and return a terminal_id without blocking — use for ANYTHING expected to take more than ~30s or that never exits on its own (dev servers, watchers, installs, long test suites, `npm run dev`, `cargo run`, `pip install`, etc.). If you omit `background`, well-known long-running commands are auto-detected and run in the background; any foreground command that exceeds 30s is stopped with a notice telling you to re-run it with background=true."
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
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "read_terminal_output".into(),
            description: "Read recent output from a background terminal (started via run_command with background=true). By default returns up to the last ~32KB of raw buffered output (includes scrollback). Set `rendered: true` to instead get the *current visible screen* as clean plain text with all escape sequences resolved by a headless terminal emulator — use this for TUIs (vim, htop, lazygit, anything that redraws in place) or heavily colorized output, where the raw buffer is full of control codes. Use the default raw mode to check progress of a long-running command — e.g. whether a dev server is up, a build finished, or a `pip install` completed.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "terminal_id": {
                        "type": "integer",
                        "description": "Session id returned by run_command(background=true)"
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
            .map(|c| {
                if c.len() > 200 {
                    // Find a safe character boundary at or before byte 200
                    let mut boundary = 200.min(c.len());
                    while boundary > 0 && !c.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    &c[..boundary]
                } else {
                    c
                }
            })
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

    let terminal_id = parse_terminal_id(&params);
    // `background` is optional. When the model omits it we default to
    // foreground, but auto-promote well-known long-running commands (dev
    // servers, watchers, installs, …) to background so they don't block the
    // chat. Foreground commands that overshoot 30s are stopped by a watchdog
    // in run_foreground (see FG_TIMEOUT) with an actionable notice.
    let background_explicit = params.get("background").and_then(|v| v.as_bool());
    let auto_background =
        background_explicit.is_none() && terminal_id.is_none() && looks_long_running(cmd_str);
    let background = background_explicit.unwrap_or(false) || auto_background;
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



    if background {
        let mut out = run_background(tool_use_id, cmd_str, cwd, terminal_id, shell, context)?;
        if auto_background && !out.is_error {
            out.content = format!(
                "AUTO_BACKGROUNDED: this looks like a long-running/persistent command, so it was started in the background instead of blocking the chat.\n{}",
                out.content
            );
        }
        return Ok(out);
    }

    let output = run_foreground(tool_use_id, cmd_str, cwd, shell.as_deref(), context)?;
    Ok(output)
}

/// Foreground commands are meant to finish quickly; anything past this is
/// stopped and the model is told to re-run with background=true.
const FG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Heuristic: does this command look like a long-running / never-exits process
/// that should run in the background by default? Intentionally conservative —
/// only matches well-known patterns so we never background a quick command the
/// model expected output from.
fn looks_long_running(cmd: &str) -> bool {
    let lower = cmd.to_ascii_lowercase();
    // Dev servers / watchers / persistent runners.
    const NEEDLES: &[&str] = &[
        "npm run dev",
        "npm start",
        "yarn dev",
        "yarn start",
        "pnpm dev",
        "pnpm start",
        "bun run dev",
        "bun dev",
        "vite",
        "next dev",
        "nodemon",
        "webpack serve",
        "webpack-dev-server",
        "ng serve",
        "rails server",
        "rails s",
        "flask run",
        "manage.py runserver",
        "uvicorn",
        "gunicorn",
        "php artisan serve",
        "http-server",
        "serve ",
        "watch",
        "--watch",
        "-w ",
        "tail -f",
        "cargo watch",
        "cargo run",
        "go run",
        "docker compose up",
        "docker-compose up",
        "tauri dev",
    ];
    if NEEDLES.iter().any(|n| lower.contains(n)) {
        return true;
    }
    // Package installs are slow but DO exit — still better backgrounded so a
    // cold cache doesn't trip the 30s foreground watchdog.
    const INSTALLS: &[&str] = &[
        "npm install",
        "npm ci",
        "yarn install",
        "pnpm install",
        "bun install",
        "pip install",
        "pip3 install",
        "poetry install",
        "cargo build",
        "cargo install",
        "gradle build",
        "mvn install",
        "apt-get install",
        "brew install",
    ];
    INSTALLS.iter().any(|n| lower.contains(n))
}

/// Resolve the default Windows shell once per process: pwsh.exe (PowerShell 7+)
/// if it's on PATH, else Windows PowerShell 5.1 at its fixed system path, else
/// cmd.exe as a last-resort fallback. Cached because PATH walks aren't free and
/// every foreground `run_command` hits this path.
#[cfg(windows)]
fn default_windows_shell() -> &'static str {
    use std::sync::OnceLock;
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            if let Some(path) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path) {
                    let candidate = dir.join("pwsh.exe");
                    if candidate.is_file() {
                        return candidate.to_string_lossy().into_owned();
                    }
                }
            }
            let legacy = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
            if std::path::Path::new(legacy).is_file() {
                return legacy.to_string();
            }
            "cmd".to_string()
        })
        .as_str()
}

fn build_shell_invocation(shell: Option<&str>, cmd_str: &str) -> (String, Vec<String>) {
    let Some(raw) = shell else {
        #[cfg(windows)]
        {
            let resolved = default_windows_shell();
            let lower = resolved.to_ascii_lowercase();
            return if lower.ends_with("cmd.exe") || lower == "cmd" {
                (resolved.to_string(), vec!["/C".into(), cmd_str.into()])
            } else {
                (
                    resolved.to_string(),
                    vec!["-NoProfile".into(), "-Command".into(), cmd_str.into()],
                )
            };
        }
        #[cfg(not(windows))]
        {
            return ("sh".into(), vec!["-c".into(), cmd_str.into()]);
        }
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
    let short_cmd = truncate_utf8(cmd_str, 57);
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
    cmd.args(&args)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    no_window(&mut cmd);

    // Run with a 30s watchdog. We spawn the child and drain stdout/stderr on
    // dedicated threads (so a chatty child can't deadlock by filling a pipe
    // buffer while we wait), then poll for exit. If the command overshoots
    // FG_TIMEOUT it almost certainly should have been a background command —
    // we kill it and tell the model to re-run with background=true rather than
    // hang the chat forever. block_in_place yields the tokio worker so other
    // async work proceeds while we poll.
    enum FgResult {
        Done(std::process::Output),
        SpawnErr(std::io::Error),
        TimedOut,
    }
    let fg = tokio::task::block_in_place(|| {
        use std::io::Read;
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return FgResult::SpawnErr(e),
        };
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut s) = stdout.take() {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });
        let err_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut s) = stderr.take() {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() >= FG_TIMEOUT {
                        let _ = child.kill();
                        let _ = child.wait();
                        // Joining now returns whatever was captured before the
                        // pipes closed on kill.
                        let _ = out_thread.join();
                        let _ = err_thread.join();
                        return FgResult::TimedOut;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return FgResult::SpawnErr(e),
            }
        };
        let stdout = out_thread.join().unwrap_or_default();
        let stderr = err_thread.join().unwrap_or_default();
        FgResult::Done(std::process::Output { status, stdout, stderr })
    });

    if let FgResult::TimedOut = fg {
        if let Some((session_id, broker)) = pty_session.as_ref() {
            let note = format!(
                "\r\n\x1b[33m[foreground timeout after {}s — re-run with background=true]\x1b[0m\r\n",
                FG_TIMEOUT.as_secs()
            );
            let _ = broker.write_raw(*session_id, &note);
            let _ = broker.kill(*session_id);
        }
        return Ok(ToolOutput {
            content: format!(
                "FOREGROUND_TIMEOUT: `{}` was still running after {}s and was stopped so it wouldn't block the chat. \
                 This command is long-running — re-run it with background=true to run it in a persistent terminal \
                 (you'll get a terminal_id; use read_terminal_output to check progress).",
                if cmd_str.len() > 200 {
                    // Find a safe character boundary at or before byte 200
                    let mut boundary = 200.min(cmd_str.len());
                    while boundary > 0 && !cmd_str.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    &cmd_str[..boundary]
                } else {
                    cmd_str
                },
                FG_TIMEOUT.as_secs()
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let output = match fg {
        FgResult::Done(out) => Ok(out),
        FgResult::SpawnErr(e) => Err(e),
        FgResult::TimedOut => unreachable!("handled above"),
    };

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

    let short_cmd = truncate_utf8(cmd_str, 57);
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

    // Auto-close after the command finishes — only for freshly-spawned
    // terminals. The shell drops to a prompt once the agent's command
    // exits, which means the pty stays open and the Terminals row never
    // disappears even after the work is done. Queuing `exit` on the next
    // line tells the shell to terminate as soon as the command above it
    // returns. EOF then fires through `spawn_output_reader` → the session
    // is destroyed → `terminal-list-changed` removes the row from the UI.
    //
    // We skip this for reuse (terminal_id passed in) because the agent is
    // explicitly opting into keeping the shell alive — e.g. a previously-
    // activated venv where follow-up commands need the same shell session.
    if created_new {
        if let Err(e) = broker.send_command(session_id, "exit") {
            tracing::warn!(
                target: "rustic::agent::terminal",
                session_id,
                error = %e,
                "failed to queue auto-exit; terminal will stay alive after the command finishes"
            );
        }
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
    let short = truncate_utf8(base, 20);
    format!("agent: {}", short)
}

/// Lenient `terminal_id` parser — accepts either a JSON integer or a string
/// that parses as an unsigned integer. We declare `terminal_id` as
/// `type: "integer"` in the tool schema, but some models (the agent in the
/// 2026-05-25 bug report being one of them) call with `{"terminal_id": "3"}`
/// anyway. Failing the call leaves the agent unable to read its own
/// background terminals; tolerating both shapes here costs nothing.
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
                is_error: true, attachments: Vec::new() });
        }
    };

    let id = match parse_terminal_id(&params) {
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

    // `rendered: true` asks the headless emulator for the current visible
    // screen (escape codes resolved); otherwise we return the raw byte tail.
    let rendered = params["rendered"].as_bool().unwrap_or(false);

    let result = if rendered {
        broker.render_screen(id)
    } else {
        broker.read_output(id, max_bytes)
    };

    match result {
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

    let id = match parse_terminal_id(&params) {
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
