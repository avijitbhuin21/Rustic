use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
use anyhow::Result;
use serde_json::{json, Value};
use tokio::process::Command;

/// Spawn the command without flashing a console window on Windows. GUI Tauri
/// processes don't own a console, so child cmd/powershell spawns briefly pop
/// one open by default. CREATE_NO_WINDOW (0x0800_0000) suppresses that.
#[cfg(windows)]
fn no_window(cmd: &mut Command) -> &mut Command {
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

/// Apply head+tail truncation to `s`, keeping at most `max_bytes` total.
/// If `s` fits within the limit it is returned unchanged. Otherwise the first
/// 4 KB and the last 12 KB are kept with a truncation marker in the middle
/// that reports how many lines were omitted. Used both for normal foreground
/// command output and for partial output captured before a timeout kill.
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
        "Run a shell command. Set `background: false` for commands that complete quickly and return output (builds, tests, git, file ops) — the output is returned to you directly. Set `background: true` for long-running or persistent processes (dev servers, watchers, `npm run dev`, `cargo run` of a server, anything that does not exit on its own) — the command runs in a persistent pty-backed terminal without blocking the chat, and you get back a `terminal_id`. \n\nTo reuse a previous background terminal (e.g. to run more commands inside an activated virtualenv or a shell session you already started), pass its `terminal_id`. Omit `terminal_id` to spawn a fresh terminal. After starting a background command, use `read_terminal_output` to check progress and `kill_terminal` when done. If you have nothing left to do until a background command finishes, simply end your turn — you are woken automatically with the command's output once it completes (never-ending processes like dev servers do not trigger this).{}",
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
            is_error: true,
            attachments: Vec::new(),
        });
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
            is_error: true,
            attachments: Vec::new(),
        });
    }

    // SECURITY: resolve `cwd` through the same canonical-prefix scope check the
    // file tools use — a bare `project_root.join(cwd)` lets an absolute path or
    // `..` traversal run the command anywhere on disk.
    let cwd = match params["cwd"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(c) => match super::file_ops::resolve_within_project(&context.project_root, c) {
            Ok(p) => p,
            Err(out) => return Ok(out),
        },
        None => context.project_root.clone(),
    };

    // SECURITY: pass the full, untruncated command to the permission broker.
    // A previous version truncated at 60 chars, letting prompt-injected commands
    // hide a malicious payload after a benign prefix (e.g. `npm test  # ; curl … | sh`).
    if context.needs_exec_approval() {
        let shell_tag = shell
            .as_deref()
            .map(|s| format!(" ({})", s))
            .unwrap_or_default();
        // Surface a non-default working directory in the approval preview so
        // the user sees where the command will actually run.
        let cwd_tag = if cwd != context.project_root {
            format!(" [cwd: {}]", cwd.display())
        } else {
            String::new()
        };
        let preview = if background {
            if let Some(id) = terminal_id {
                format!(
                    "[background in terminal #{}]{} {}{}",
                    id, shell_tag, cmd_str, cwd_tag
                )
            } else {
                format!(
                    "[background, new terminal]{} {}{}",
                    shell_tag, cmd_str, cwd_tag
                )
            }
        } else {
            format!("{}{}{}", shell_tag, cmd_str, cwd_tag)
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
                attachments: Vec::new(),
            });
        }
    }

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

    let output = run_foreground(tool_use_id, cmd_str, cwd, shell.as_deref(), context).await?;
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

/// Resolve a usable bash on Windows. Git-for-Windows FIRST: the
/// `C:\Windows\System32\bash.exe` found on PATH is the WSL launcher and hard-
/// errors on machines without a WSL distro/VM ("WSL2 is not supported with
/// your current machine configuration"). PATH is only consulted last, and the
/// System32 stub is skipped.
#[cfg(windows)]
fn windows_bash_path() -> Option<String> {
    for candidate in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ] {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            let git = std::path::Path::new(dir).join("git.exe");
            if git.exists() {
                if let Some(root) = std::path::Path::new(dir).parent() {
                    let bash = root.join("bin").join("bash.exe");
                    if bash.exists() {
                        return Some(bash.to_string_lossy().into_owned());
                    }
                }
            }
        }
        for dir in path_var.split(';') {
            let p = std::path::Path::new(dir).join("bash.exe");
            if p.exists()
                && !p
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("system32")
            {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
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
        "bash" | "sh" => {
            #[cfg(windows)]
            {
                let is_bare_name = !raw.contains('/') && !raw.contains('\\');
                if is_bare_name {
                    if let Some(p) = windows_bash_path() {
                        return (p, vec!["-c".into(), cmd_str.into()]);
                    }
                }
            }
            (raw.to_string(), vec!["-c".into(), cmd_str.into()])
        }
        _ => (raw.to_string(), vec!["-c".into(), cmd_str.into()]),
    }
}

async fn run_foreground(
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

    if let (Some(history), Some(message_id)) = (
        context.file_history.as_ref(),
        context.current_user_message_id.as_ref(),
    ) {
        let _ = history.checkpoint_pre_bash(message_id);
    }

    let pty_session: Option<(u64, std::sync::Arc<dyn crate::AgentTerminals>)> =
        if let Some(broker) = context.agent_terminals.as_ref() {
            let label = format!("$ {short_cmd}");
            match broker.spawn(cwd.clone(), label, &context.task_id, None) {
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

    // Run with a 30s watchdog, fully async — no blocked runtime thread and no
    // 50ms try_wait poll loop. stdout/stderr are drained by spawned tasks (so
    // a chatty child can't deadlock by filling a pipe buffer while we wait),
    // and the watchdog is a tokio::time::timeout around child.wait(). If the
    // command overshoots FG_TIMEOUT it almost certainly should have been a
    // background command — we kill it and tell the model to re-run with
    // background=true rather than hang the chat forever.
    enum FgResult {
        Done(std::process::Output),
        SpawnErr(std::io::Error),
        /// Command was killed after FG_TIMEOUT. Carries whatever stdout/stderr
        /// was captured before the pipes closed so we can return it to the model.
        TimedOut {
            stdout: Vec<u8>,
            stderr: Vec<u8>,
        },
    }
    let fg = match cmd.spawn() {
        Err(e) => FgResult::SpawnErr(e),
        Ok(mut child) => {
            use tokio::io::AsyncReadExt;
            let mut stdout = child.stdout.take();
            let mut stderr = child.stderr.take();
            // Reader tasks are joined with a bounded timeout on the kill path
            // rather than awaited unconditionally: after a timeout kill, a
            // grandchild that inherited the pipe can keep it open
            // indefinitely, so a bare await on the reader could hang the
            // watchdog path forever. A still-blocked reader task is simply
            // abandoned (it completes once the pipe finally closes).
            let out_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut s) = stdout.take() {
                    let _ = s.read_to_end(&mut buf).await;
                }
                buf
            });
            let err_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut s) = stderr.take() {
                    let _ = s.read_to_end(&mut buf).await;
                }
                buf
            });
            match tokio::time::timeout(FG_TIMEOUT, child.wait()).await {
                Ok(Ok(status)) => {
                    let stdout = out_task.await.unwrap_or_default();
                    let stderr = err_task.await.unwrap_or_default();
                    FgResult::Done(std::process::Output {
                        status,
                        stdout,
                        stderr,
                    })
                }
                Ok(Err(e)) => FgResult::SpawnErr(e),
                Err(_elapsed) => {
                    // kill() both signals the child and reaps it (start_kill + wait).
                    let _ = child.kill().await;
                    // Collect whatever was buffered before the kill, but never
                    // wait more than 2s per pipe (see comment above).
                    const DRAIN: std::time::Duration = std::time::Duration::from_secs(2);
                    let partial_stdout = tokio::time::timeout(DRAIN, out_task)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                    let partial_stderr = tokio::time::timeout(DRAIN, err_task)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                    FgResult::TimedOut {
                        stdout: partial_stdout,
                        stderr: partial_stderr,
                    }
                }
            }
        }
    };

    if let FgResult::TimedOut {
        stdout: partial_out,
        stderr: partial_err,
    } = fg
    {
        if let Some((session_id, broker)) = pty_session.as_ref() {
            let note = format!(
                "\r\n\x1b[33m[foreground timeout after {}s — re-run with background=true]\x1b[0m\r\n",
                FG_TIMEOUT.as_secs()
            );
            let _ = broker.write_raw(*session_id, &note);
            let _ = broker.kill(*session_id);
        }
        let short_cmd = if cmd_str.len() > 200 {
            // Find a safe character boundary at or before byte 200
            let mut boundary = 200.min(cmd_str.len());
            while boundary > 0 && !cmd_str.is_char_boundary(boundary) {
                boundary -= 1;
            }
            &cmd_str[..boundary]
        } else {
            cmd_str
        };
        let mut content = format!(
            "FOREGROUND_TIMEOUT: `{}` was still running after {}s and was stopped so it wouldn't block the chat. \
             This command is long-running — re-run it with background=true to run it in a persistent terminal \
             (you'll get a terminal_id; use read_terminal_output to check progress).",
            short_cmd,
            FG_TIMEOUT.as_secs()
        );
        // Append whatever was captured on stdout/stderr before the kill.
        let partial_stdout = String::from_utf8_lossy(&partial_out);
        let partial_stderr = String::from_utf8_lossy(&partial_err);
        let mut partial = String::new();
        if !partial_stdout.is_empty() {
            partial.push_str(&partial_stdout);
        }
        if !partial_stderr.is_empty() {
            if !partial.is_empty() {
                partial.push_str("\n--- stderr ---\n");
            }
            partial.push_str(&partial_stderr);
        }
        if !partial.is_empty() {
            let partial_display = format_output_head_tail(&partial, FG_OUTPUT_MAX_BYTES);
            content.push_str("\n\n--- partial output before timeout ---\n");
            content.push_str(&partial_display);
        }
        return Ok(ToolOutput {
            content,
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let output = match fg {
        FgResult::Done(out) => Ok(out),
        FgResult::SpawnErr(e) => Err(e),
        FgResult::TimedOut { .. } => unreachable!("handled above"),
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

            let result = format_output_head_tail(&result, FG_OUTPUT_MAX_BYTES);

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
            format!(
                "{}\r\n\x1b[31m[exit: error]\x1b[0m\r\n",
                tool_output.content
            )
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
                is_error: true,
                attachments: Vec::new(),
            });
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
    context.emit_progress(tool_use_id, &format!("$ [bg#{session_id}] {short_cmd}"));

    if let Err(e) = broker.send_command(session_id, cmd_str) {
        return Ok(ToolOutput {
            content: format!(
                "Started terminal #{}, but failed to send command: {}",
                session_id, e
            ),
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

    // ── timeout result structure (logic tests) ───────────────────────────────
    // We verify the message-assembly logic that run_foreground uses for the
    // timeout case by reproducing the same pattern and asserting structure.

    #[test]
    fn timeout_message_has_error_code_prefix() {
        let cmd_str = "sleep 60";
        let partial = "some output line\n";
        let partial_display = format_output_head_tail(partial, FG_OUTPUT_MAX_BYTES);

        let mut content = format!(
            "FOREGROUND_TIMEOUT: `{}` was still running after {}s and was stopped so it wouldn't block the chat. \
             This command is long-running — re-run it with background=true to run it in a persistent terminal \
             (you'll get a terminal_id; use read_terminal_output to check progress).",
            cmd_str,
            FG_TIMEOUT.as_secs(),
        );
        content.push_str("\n\n--- partial output before timeout ---\n");
        content.push_str(&partial_display);

        assert!(
            content.starts_with("FOREGROUND_TIMEOUT:"),
            "error code must be first token"
        );
        assert!(content.contains("--- partial output before timeout ---"));
        assert!(content.contains("some output line"));
    }

    #[test]
    fn timeout_no_partial_section_when_empty_output() {
        // When stdout and stderr are both empty, no partial section is emitted.
        let partial_stdout = "";
        let partial_stderr = "";
        let mut partial = String::new();
        if !partial_stdout.is_empty() {
            partial.push_str(partial_stdout);
        }
        if !partial_stderr.is_empty() {
            if !partial.is_empty() {
                partial.push_str("\n--- stderr ---\n");
            }
            partial.push_str(partial_stderr);
        }

        let mut content = "FOREGROUND_TIMEOUT: command timed out.".to_string();
        if !partial.is_empty() {
            let partial_display = format_output_head_tail(&partial, FG_OUTPUT_MAX_BYTES);
            content.push_str("\n\n--- partial output before timeout ---\n");
            content.push_str(&partial_display);
        }

        assert!(
            !content.contains("--- partial output before timeout ---"),
            "partial section should not appear when there is no output"
        );
    }

    #[test]
    fn timeout_partial_includes_stderr_after_stdout() {
        let partial_stdout = "stdout line\n";
        let partial_stderr = "stderr line\n";
        let mut partial = String::new();
        partial.push_str(partial_stdout);
        partial.push_str("\n--- stderr ---\n");
        partial.push_str(partial_stderr);

        let display = format_output_head_tail(&partial, FG_OUTPUT_MAX_BYTES);
        assert!(display.contains("stdout line"));
        assert!(display.contains("--- stderr ---"));
        assert!(display.contains("stderr line"));
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
}
