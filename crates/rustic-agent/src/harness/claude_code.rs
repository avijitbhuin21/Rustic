//! `Harness` implementation for the Claude Code CLI.
//!
//! Spawns `claude --print --output-format stream-json --input-format stream-json`,
//! then runs two background tasks:
//!
//! * **Reader** — pulls NDJSON envelopes off stdout, translates each one
//!   through `event_map`, and pushes the resulting `HarnessEvent`s onto an
//!   mpsc the host code consumes.
//! * **Stderr drain** — keeps the OS pipe from blocking and stashes a tail
//!   we can include in the final error if the CLI crashes.
//!
//! Stdin is owned by the session and only written to from caller-driven
//! methods (`send_user_message`, `respond_to_permission`, `interrupt`),
//! serialised through the `NdjsonWriter`'s mutex.
//!
//! Chunk 2 scope: only `send_user_message` is wired through end-to-end.
//! Permission / question / interrupt methods are stubbed with a clear
//! `not yet implemented` error so a misbehaving caller surfaces fast
//! instead of silently no-op'ing.

use crate::harness::auth_check::{probe_claude_code, HarnessAuthStatus};
use crate::harness::event_map::translate_claude_envelope;
use crate::harness::process_spawn::{HarnessSpawnSpec, SpawnedHarnessChild};
use crate::harness::stream_json::{NdjsonReader, NdjsonWriter};
use crate::harness::{
    Harness, HarnessEvent, HarnessImage, HarnessKind, HarnessPermissionMode, HarnessSession,
    HarnessSessionOpts, PermissionDecision,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

/// Stateless factory. Construct once, call `start_session` per chat task.
pub struct ClaudeCodeHarness;

impl ClaudeCodeHarness {
    pub fn new() -> Self {
        Self
    }

    /// Convenience: probe the binary so callers can show a clean error before
    /// even attempting to spawn a session.
    pub async fn probe(binary_override: Option<&std::path::Path>) -> HarnessAuthStatus {
        probe_claude_code(binary_override).await
    }
}

impl Default for ClaudeCodeHarness {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Harness for ClaudeCodeHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
    }

    async fn start_session(
        &self,
        opts: HarnessSessionOpts,
    ) -> Result<Arc<dyn HarnessSession>> {
        let session = ClaudeCodeSession::spawn(opts).await?;
        Ok(Arc::new(session))
    }
}

/// A live session bound to one running `claude` child process.
pub struct ClaudeCodeSession {
    /// Child stdin, wrapped in the writer's mutex so the user-message,
    /// permission-response, and interrupt paths can serialise without tearing.
    writer: NdjsonWriter<ChildStdin>,
    /// Receiver for translated events. Taken once by the host runtime via
    /// `take_event_rx`; subsequent calls return `None`.
    event_rx: AsyncMutex<Option<mpsc::UnboundedReceiver<HarnessEvent>>>,
    /// Latest CLI-reported session ID, captured from the first `system:init`
    /// envelope. Persisted to `tasks.harness_session_id` for resume.
    session_id: AsyncMutex<Option<String>>,
    /// Owns the child handle (Job Object on Windows ensures kill-on-drop).
    /// Wrapped in `AsyncMutex<Option<...>>` so `shutdown` can take + kill +
    /// drop while remaining idempotent across concurrent callers.
    child: AsyncMutex<Option<SpawnedHarnessChild>>,
    /// Tail of stderr (last ~64 KB), captured by the drain task. Surfaced in
    /// the final error message when the child exits unexpectedly.
    stderr_tail: Arc<AsyncMutex<String>>,
    /// In-flight permission prompts: `request_id` → `tool_name`. Populated by
    /// the reader task when it sees a `can_use_tool` control_request, drained
    /// by `respond_to_permission`. We keep it here (rather than asking the
    /// host to thread tool_name through) so the trait surface stays minimal.
    pending_permission_requests: Arc<AsyncMutex<HashMap<String, String>>>,
    /// Most recent moment we saw activity on this session — bumped by the
    /// reader task on every envelope and by `send_user_message` when the
    /// host writes user input. Read by `HarnessRegistry::reap_idle` to
    /// decide whether to drop the CLI process. Plan §B.5.
    last_active: Arc<AsyncMutex<Instant>>,
}

impl ClaudeCodeSession {
    async fn spawn(opts: HarnessSessionOpts) -> Result<Self> {
        let program = opts
            .binary_path_override
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "claude".to_string());

        let args = build_spawn_args(&opts);

        let spec = HarnessSpawnSpec {
            program,
            args,
            // OS-level cwd. Pass project root so `claude`'s relative-path
            // tools see the same root as the user.
            cwd: opts.cwd.clone(),
            env: vec![],
        };

        let mut child = SpawnedHarnessChild::spawn(spec)
            .context("failed to spawn `claude` CLI")?;

        // Take ownership of the stdio. The child struct stays in self.child
        // so its Drop kills the process (and the Job Object on Windows kills
        // the Node descendants) when the session ends.
        let stdin = child
            .stdin
            .take()
            .context("claude CLI stdin missing immediately after spawn")?;
        let stdout = child
            .stdout
            .take()
            .context("claude CLI stdout missing immediately after spawn")?;
        let stderr = child
            .stderr
            .take()
            .context("claude CLI stderr missing immediately after spawn")?;

        let writer = NdjsonWriter::new(stdin);
        let (event_tx, event_rx) = mpsc::unbounded_channel::<HarnessEvent>();
        let stderr_tail = Arc::new(AsyncMutex::new(String::new()));
        let pending_permission_requests: Arc<AsyncMutex<HashMap<String, String>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));
        let last_active: Arc<AsyncMutex<Instant>> = Arc::new(AsyncMutex::new(Instant::now()));

        // Reader task: NDJSON envelopes → HarnessEvent → mpsc.
        // Also stashes `(request_id → tool_name)` for permission requests so
        // `respond_to_permission` can build a session-allowlist rule when
        // the user picks AcceptForSession.
        let reader_tx = event_tx.clone();
        let pending_for_reader = Arc::clone(&pending_permission_requests);
        let last_active_for_reader = Arc::clone(&last_active);
        tokio::spawn(async move {
            let mut reader = NdjsonReader::new(stdout);
            loop {
                match reader.next_envelope().await {
                    Ok(None) => {
                        // Clean EOF — CLI exited normally after a `result`.
                        // The host runtime relies on `TurnComplete` having
                        // fired from the `result` envelope to mark the task
                        // done; nothing extra to emit here.
                        break;
                    }
                    Ok(Some(env)) => {
                        // Bump idle clock on any received envelope — the CLI
                        // is alive and producing output, so the reaper should
                        // leave this session alone (plan §B.5).
                        *last_active_for_reader.lock().await = Instant::now();
                        for ev in translate_claude_envelope(&env) {
                            if let HarnessEvent::PermissionRequest {
                                request_id,
                                tool_name,
                                ..
                            } = &ev
                            {
                                pending_for_reader
                                    .lock()
                                    .await
                                    .insert(request_id.clone(), tool_name.clone());
                            }
                            if reader_tx.send(ev).is_err() {
                                // Receiver dropped — host runtime is gone.
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = reader_tx.send(HarnessEvent::Error {
                            message: format!("stream-json parse error: {e:#}"),
                        });
                        // Don't break — a single malformed line shouldn't
                        // kill the whole session. The CLI may recover.
                    }
                }
            }
        });

        // Stderr drain. We keep up to ~64 KB; older bytes get truncated from
        // the front so the buffer doesn't grow unbounded on a chatty CLI.
        let stderr_tail_for_drain = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            const MAX_TAIL: usize = 64 * 1024;
            let mut buf = [0u8; 4096];
            let mut reader = BufReader::new(stderr);
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut g = stderr_tail_for_drain.lock().await;
                        g.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if g.len() > MAX_TAIL {
                            let drop_n = g.len() - MAX_TAIL;
                            *g = g[drop_n..].to_string();
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            writer,
            event_rx: AsyncMutex::new(Some(event_rx)),
            session_id: AsyncMutex::new(None),
            child: AsyncMutex::new(Some(child)),
            stderr_tail,
            pending_permission_requests,
            last_active,
        })
    }

    /// Look up the tool name we recorded when this permission request first
    /// arrived. Used by `respond_to_permission` to build the session-rule
    /// when the user picks `AcceptForSession`.
    async fn last_request_tool_name(&self, request_id: &str) -> Option<String> {
        self.pending_permission_requests
            .lock()
            .await
            .get(request_id)
            .cloned()
    }

    async fn forget_request(&self, request_id: &str) {
        self.pending_permission_requests
            .lock()
            .await
            .remove(request_id);
    }
}

#[async_trait]
impl HarnessSession for ClaudeCodeSession {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
    }

    async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    async fn send_user_message(&self, text: String, images: Vec<HarnessImage>) -> Result<()> {
        // Bump idle clock — user activity counts even before the CLI
        // responds, so a long-thinking turn doesn't get reaped between
        // user-send and the first delta.
        *self.last_active.lock().await = Instant::now();

        let envelope = build_user_envelope(&text, &images);
        self.writer.write(&envelope).await
    }

    async fn respond_to_permission(
        &self,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<()> {
        // We don't track the original `tool_use_id` / `input` here — when
        // the user accepts, we send `updatedInput: {}` which Claude Code
        // interprets as "use the original tool input" (per
        // PermissionPromptToolResultSchema.ts:110 — empty object means
        // "fall back to the original arguments"). That's what we want; we'd
        // only need to override the input if Rustic let the user edit the
        // tool call before approving, which we don't.
        let inner = match decision {
            PermissionDecision::Accept => serde_json::json!({
                "behavior": "allow",
                "updatedInput": {},
            }),
            PermissionDecision::AcceptForSession => {
                // The CLI exposes a `permissionUpdate` mechanism via the
                // response: an `addRules` entry with `destination: "session"`
                // grants the rule for the rest of the run. We grant by
                // tool name only (no `ruleContent`) — matches the "allow
                // this tool, no matter the arguments" semantic users
                // expect from the middle button. Refining to per-input
                // rules requires per-tool logic and is plan §6.1 territory.
                let tool_name = self.last_request_tool_name(&request_id).await;
                let mut rules = Vec::new();
                if let Some(name) = tool_name {
                    rules.push(serde_json::json!({ "toolName": name }));
                }
                let updated_permissions = if rules.is_empty() {
                    serde_json::Value::Array(Vec::new())
                } else {
                    serde_json::json!([
                        {
                            "type": "addRules",
                            "rules": rules,
                            "behavior": "allow",
                            "destination": "session",
                        }
                    ])
                };
                serde_json::json!({
                    "behavior": "allow",
                    "updatedInput": {},
                    "updatedPermissions": updated_permissions,
                })
            }
            PermissionDecision::Deny => serde_json::json!({
                "behavior": "deny",
                "message": "User declined tool execution.",
            }),
        };

        let envelope = serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": inner,
            },
        });
        self.writer.write(&envelope).await?;
        // Drop the request-tracking entry — the CLI won't ask about this id
        // again (it'll mint a new one for the next prompt).
        self.forget_request(&request_id).await;
        Ok(())
    }

    async fn respond_to_question(&self, _request_id: String, _answer: String) -> Result<()> {
        Err(anyhow!(
            "respond_to_question is not yet implemented for ClaudeCodeSession"
        ))
    }

    async fn interrupt(&self) -> Result<()> {
        // Stream-json's `control_request` with `subtype: interrupt` aborts
        // the in-flight turn. The CLI replies with a `control_response`
        // success that we ignore (it's just an ack), and then emits a
        // normal `result` envelope so the host runtime sees `TurnComplete`
        // and the registry entry gets cleaned up the next time it idles.
        //
        // The escalation ladder from plan §13.5 (write → SIGINT → SIGKILL)
        // is chunk 4c. For now: best-effort write, fall through to caller's
        // own kill-switch (the cancel token + `shutdown` for hard-kill) if
        // the CLI is unresponsive.
        let envelope = serde_json::json!({
            "type": "control_request",
            "request_id": uuid::Uuid::new_v4().to_string(),
            "request": { "subtype": "interrupt" },
        });
        self.writer.write(&envelope).await
    }

    async fn shutdown(&self) -> Result<()> {
        let mut g = self.child.lock().await;
        if let Some(mut child) = g.take() {
            // Best-effort kill. Drop also fires kill_on_drop + Job Object,
            // but await'ing kill ensures we don't return before the OS has
            // actually reaped it.
            let _ = child.kill().await;
        }
        Ok(())
    }

    async fn take_event_rx(&self) -> Option<mpsc::UnboundedReceiver<HarnessEvent>> {
        self.event_rx.lock().await.take()
    }

    async fn stderr_tail(&self) -> String {
        self.stderr_tail.lock().await.clone()
    }

    async fn last_active(&self) -> Instant {
        *self.last_active.lock().await
    }
}


/// Build the argv passed to `claude --print ...`. Extracted as a free fn so
/// the spawn surface is unit-testable without hitting the OS (plan §B.12 —
/// MCP passthrough adds an extra branch we want pinned by tests).
fn build_spawn_args(opts: &HarnessSessionOpts) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--input-format".into(),
        "stream-json".into(),
        // The CLI refuses `--print --output-format=stream-json` without
        // `--verbose` — it bails with `Error: When using --print,
        // --output-format=stream-json requires --verbose`. The flag just
        // unlocks the streaming output; it doesn't add noise to our
        // NDJSON stream because the envelopes themselves are unchanged.
        "--verbose".into(),
        // Include the underlying Anthropic-API `stream_event` envelopes
        // so we can do real character-by-character text streaming. Without
        // this flag, the CLI only emits one consolidated `assistant`
        // envelope per turn, which makes the chat feel laggy.
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        permission_mode_flag(opts.permission_mode).into(),
    ];

    // Working directory: Claude Code derives its project scope from the
    // OS-level cwd of the spawned process (which we set via
    // `HarnessSpawnSpec.cwd`). The CLI does **not** accept a `--cwd` flag;
    // recent versions reject it as `unknown option '--cwd'`. Don't add one.

    if let Some(resume_id) = &opts.resume_session_id {
        args.push("--resume".into());
        args.push(resume_id.clone());
    }

    // Model selection from the picker. Claude Code accepts either an alias
    // (`sonnet`, `opus`, `haiku` — auto-resolves to latest) or a full name
    // (e.g. `claude-sonnet-4-6`). The user can also override mid-session
    // via the `/model` slash command — that takes precedence over this
    // flag, which is fine because the CLI tracks its own active model.
    if let Some(model) = &opts.model {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            args.push("--model".into());
            args.push(trimmed.to_string());
        }
    }

    // Thinking / reasoning-effort tier from the agent-config popover.
    // Claude Code's `--effort` flag accepts `low | medium | high | xhigh |
    // max`; we trust the picker to send one of those (the JS layer's
    // `getThinkingCapability` already restricts the choices). Anything
    // else gets passed through verbatim — if the CLI rejects it, the
    // error surfaces in the chat which is better than silently dropping
    // a user-visible setting.
    if let Some(effort) = &opts.thinking_effort {
        let trimmed = effort.trim();
        // The picker emits "off" when thinking is disabled; the CLI has no
        // off-switch flag so the cleanest "off" is to omit `--effort`
        // entirely (the model then uses its built-in default, which is the
        // closest match for "let the model decide").
        if !trimmed.is_empty() && trimmed != "off" && trimmed != "none" {
            args.push("--effort".into());
            args.push(trimmed.to_string());
        }
    }

    // MCP passthrough (plan §B.12). Claude Code's `--mcp-config` flag
    // accepts a JSON file in the same `{"mcpServers": {...}}` shape Rustic
    // already writes, so we point the CLI straight at our user-scope file
    // and let it inherit those servers. Project-scope `.mcp.json` is
    // auto-discovered by the CLI from `--cwd` so we don't pass it.
    //
    // We avoid `--strict-mcp-config` deliberately: that flag would make
    // Claude Code ignore its own user-scope MCP config from `~/.claude.json`
    // (servers added via `claude mcp add`). Without it, the file we point at
    // is *additive* — Rustic-managed servers stack on top of CLI-managed
    // ones, with the file's entries winning on name collision.
    //
    // Skip silently when the file doesn't exist (no Rustic-managed servers
    // configured yet) so the user doesn't see a CLI error about a missing
    // config file just because they haven't opened the MCP panel.
    if let Some(path) = &opts.mcp_config_path {
        if path.exists() {
            args.push("--mcp-config".into());
            args.push(path.to_string_lossy().to_string());
        }
    }

    args
}

fn permission_mode_flag(mode: HarnessPermissionMode) -> &'static str {
    match mode {
        HarnessPermissionMode::ReadOnly => "plan",
        HarnessPermissionMode::Supervised => "ifNeeded",
        HarnessPermissionMode::AcceptEdits => "acceptEdits",
        HarnessPermissionMode::BypassPermissions => "bypassPermissions",
    }
}

/// Build the stream-json `user` envelope the CLI's stdin expects.
///
/// Shape mirrors the Anthropic Messages API content-block protocol so the
/// CLI passes it straight through to the model without re-encoding:
/// ```json
/// {
///   "type": "user",
///   "message": {
///     "role": "user",
///     "content": [
///       { "type": "text",  "text": "..." },
///       { "type": "image", "source": {"type":"base64","media_type":"image/png","data":"..."} }
///     ]
///   }
/// }
/// ```
///
/// An empty `text` is dropped (don't emit a zero-length text block — the
/// API rejects them) but at least one image will still produce a valid
/// envelope. Images may follow text in any order; we put text first so the
/// transcript reads naturally if both are present.
///
/// Extracted as a free fn so the shape can be unit-tested without spawning
/// a real CLI process — see `tests::user_envelope_with_image_block`
/// (plan §B.8 verification).
fn build_user_envelope(text: &str, images: &[HarnessImage]) -> serde_json::Value {
    let mut content: Vec<serde_json::Value> = Vec::with_capacity(
        if text.is_empty() { 0 } else { 1 } + images.len(),
    );
    if !text.is_empty() {
        content.push(serde_json::json!({ "type": "text", "text": text }));
    }
    for img in images {
        content.push(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": img.media_type,
                "data": img.data,
            },
        }));
    }
    serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": content,
        },
    })
}

/// Build a placeholder `cwd` for the Global orchestrator chat (plan §13.2).
/// Claude Code requires a `--cwd`; we use a per-user scratch dir so it has
/// somewhere to "live" without mixing with real project files.
pub fn global_scratch_dir(app_data_dir: &std::path::Path) -> PathBuf {
    app_data_dir.join("rustic").join("global-scratch")
}

#[cfg(test)]
mod tests {
    //! Envelope-shape tests for the stream-json input the CLI consumes on
    //! stdin. The actual CLI process isn't spawned here — we just verify
    //! the JSON we'd write exactly matches Anthropic's content-block
    //! protocol so a future serializer change can't silently break image
    //! forwarding (plan §B.8).

    use super::{
        build_spawn_args, build_user_envelope, HarnessImage, HarnessPermissionMode,
        HarnessSessionOpts,
    };
    use serde_json::json;
    use std::path::PathBuf;

    fn base_opts() -> HarnessSessionOpts {
        HarnessSessionOpts {
            cwd: PathBuf::from("/projects/demo"),
            permission_mode: HarnessPermissionMode::AcceptEdits,
            resume_session_id: None,
            binary_path_override: None,
            mcp_config_path: None,
            model: None,
            thinking_effort: None,
        }
    }

    #[test]
    fn spawn_args_omit_effort_when_unset() {
        let args = build_spawn_args(&base_opts());
        assert!(!args.iter().any(|a| a == "--effort"));
    }

    #[test]
    fn spawn_args_pass_effort_when_set() {
        let mut opts = base_opts();
        opts.thinking_effort = Some("high".into());
        let args = build_spawn_args(&opts);
        let idx = args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort flag present");
        assert_eq!(args[idx + 1], "high");
    }

    #[test]
    fn spawn_args_skip_off_effort() {
        // The picker writes "off" when the user disables thinking. The CLI
        // has no off-switch flag, so omitting `--effort` lets the model
        // fall back to its default — closest match for "let it decide".
        let mut opts = base_opts();
        opts.thinking_effort = Some("off".into());
        let args = build_spawn_args(&opts);
        assert!(!args.iter().any(|a| a == "--effort"));
    }

    #[test]
    fn spawn_args_skip_blank_effort() {
        let mut opts = base_opts();
        opts.thinking_effort = Some("   ".into());
        let args = build_spawn_args(&opts);
        assert!(!args.iter().any(|a| a == "--effort"));
    }

    #[test]
    fn spawn_args_omit_model_when_unset() {
        let args = build_spawn_args(&base_opts());
        assert!(
            !args.iter().any(|a| a == "--model"),
            "no --model when model is None: {args:?}"
        );
    }

    #[test]
    fn spawn_args_pass_model_when_set() {
        let mut opts = base_opts();
        opts.model = Some("sonnet".into());
        let args = build_spawn_args(&opts);
        let idx = args
            .iter()
            .position(|a| a == "--model")
            .expect("--model flag present");
        assert_eq!(args[idx + 1], "sonnet");
    }

    #[test]
    fn spawn_args_skip_blank_model() {
        // A whitespace-only string from a misconfigured picker shouldn't
        // produce `--model "   "` — the CLI would reject it. Drop instead.
        let mut opts = base_opts();
        opts.model = Some("   ".into());
        let args = build_spawn_args(&opts);
        assert!(!args.iter().any(|a| a == "--model"), "blank model dropped");
    }

    #[test]
    fn spawn_args_omit_mcp_config_when_path_unset() {
        let args = build_spawn_args(&base_opts());
        assert!(
            !args.iter().any(|a| a == "--mcp-config"),
            "no --mcp-config when mcp_config_path is None: {args:?}"
        );
    }

    #[test]
    fn spawn_args_omit_mcp_config_when_file_missing() {
        // A non-existent path is the "user hasn't configured MCP yet" case;
        // the harness must not pass the flag, otherwise the CLI errors out
        // on a missing config file before the first turn even starts.
        let mut opts = base_opts();
        opts.mcp_config_path = Some(PathBuf::from(
            "/definitely/does/not/exist/rustic-mcp.json",
        ));
        let args = build_spawn_args(&opts);
        assert!(
            !args.iter().any(|a| a == "--mcp-config"),
            "no --mcp-config when path doesn't exist on disk: {args:?}"
        );
    }

    #[test]
    fn spawn_args_pass_mcp_config_when_file_exists() {
        // Create a real temp file so the existence check fires. Contents
        // don't matter for the args build — Claude Code does its own parse.
        let dir = std::env::temp_dir().join(format!(
            "rustic-mcp-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("mcp.json");
        std::fs::write(&path, "{\"mcpServers\": {}}").expect("write file");

        let mut opts = base_opts();
        opts.mcp_config_path = Some(path.clone());
        let args = build_spawn_args(&opts);

        let idx = args
            .iter()
            .position(|a| a == "--mcp-config")
            .expect("--mcp-config flag present");
        assert_eq!(args[idx + 1], path.to_string_lossy().to_string());

        // Cleanup — best-effort; test still passes if the temp file lingers.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn user_envelope_text_only() {
        let env = build_user_envelope("hello", &[]);
        assert_eq!(
            env,
            json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "hello" }
                    ]
                }
            })
        );
    }

    #[test]
    fn user_envelope_with_image_block() {
        let img = HarnessImage {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
        };
        let env = build_user_envelope("look at this", &[img]);
        assert_eq!(
            env,
            json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "look at this" },
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "iVBORw0KGgo="
                            }
                        }
                    ]
                }
            })
        );
    }

    #[test]
    fn user_envelope_image_only_drops_empty_text() {
        // Empty `text` must not produce a zero-length text block — the API
        // rejects empty text blocks. Images-only is a valid send (e.g. the
        // user pasted a screenshot with no caption).
        let img = HarnessImage {
            media_type: "image/jpeg".into(),
            data: "/9j/4AAQ".into(),
        };
        let env = build_user_envelope("", &[img]);
        let content = env
            .pointer("/message/content")
            .and_then(|v| v.as_array())
            .expect("content array present");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/jpeg");
    }

    #[test]
    fn user_envelope_multiple_images_preserve_order() {
        let images = vec![
            HarnessImage {
                media_type: "image/png".into(),
                data: "first".into(),
            },
            HarnessImage {
                media_type: "image/png".into(),
                data: "second".into(),
            },
        ];
        let env = build_user_envelope("two pics", &images);
        let content = env
            .pointer("/message/content")
            .and_then(|v| v.as_array())
            .expect("content array present");
        assert_eq!(content.len(), 3); // text + 2 images
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["source"]["data"], "first");
        assert_eq!(content[2]["source"]["data"], "second");
    }
}
