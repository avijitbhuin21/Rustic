//! `Harness` implementation for the Codex CLI (`codex app-server`).
//!
//! Codex's stdio transport is JSON-RPC 2.0 over newline-delimited JSON
//! (default `--listen stdio://`). The protocol is fully self-documented —
//! `codex app-server generate-json-schema --out <dir>` dumps every method
//! and notification shape; the dump committed under `docs/codex-schema/`
//! is what this implementation targets (Codex CLI 0.125.x).
//!
//! # Lifecycle
//!
//! 1. Spawn `codex app-server`. Default listen mode is `stdio://`, so
//!    JSON-RPC frames flow over stdin/stdout.
//! 2. Send `initialize` request → wait for response.
//! 3. Send `thread/start` (or `thread/resume` if a `resume_session_id`
//!    was provided) → response contains the `Thread` payload; we capture
//!    `thread.id` as our `session_id`.
//! 4. From here on, the session is a request/notification stream:
//!    - `turn/start` → starts a turn with user input.
//!    - `turn/interrupt` → aborts the active turn.
//!    - Server emits notifications for each item (`item/started`,
//!      `item/agentMessage/delta`, `item/completed`, etc.) and for turn
//!      lifecycle (`turn/started`, `turn/completed`).
//!    - Server may also send approval requests (`applyPatch`,
//!      `commandExecution`, `dynamicToolCall`) which we forward to the
//!      host as `HarnessEvent::PermissionRequest`.
//!
//! # Status
//!
//! Phase 1 of B.10: streaming text + thread lifecycle work end-to-end.
//! Permission/approval flow is a follow-up — when an inbound server
//! request arrives, we currently reply with a method-not-found error so
//! the CLI knows we declined. The host runtime then surfaces the error
//! in chat so the user sees the gap rather than a silent hang.

use crate::harness::event_map_codex::translate_codex_notification;
use crate::harness::jsonrpc::{self, encode_response, JsonRpcMessage, PendingRequests, RequestId};
use crate::harness::process_spawn::{HarnessSpawnSpec, SpawnedHarnessChild};
use crate::harness::stream_json::{NdjsonReader, NdjsonWriter};
use crate::harness::{
    Harness, HarnessEvent, HarnessImage, HarnessKind, HarnessPermissionMode, HarnessSession,
    HarnessSessionOpts, PermissionDecision,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

/// Stateless factory; matches the `ClaudeCodeHarness` shape so the
/// dispatch branch in `harness_runtime.rs` is symmetrical.
pub struct CodexHarness;

impl CodexHarness {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexHarness {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Harness for CodexHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
    }

    async fn start_session(
        &self,
        opts: HarnessSessionOpts,
    ) -> Result<Arc<dyn HarnessSession>> {
        let session = CodexSession::spawn(opts).await?;
        Ok(Arc::new(session))
    }
}

/// Live session bound to one running `codex app-server` child process.
pub struct CodexSession {
    /// Child stdin wrapped so `turn/start`, `turn/interrupt`, and
    /// approval responses serialise without tearing.
    writer: NdjsonWriter<ChildStdin>,
    /// Pending outbound JSON-RPC requests waiting on responses.
    pending: Arc<PendingRequests>,
    /// Per-thread model + effort the picker recorded at session-start
    /// time. We forward these on every `turn/start` so the user's
    /// runtime selection actually drives the model — `thread/start.model`
    /// alone wouldn't follow mid-conversation switches.
    pinned_model: Option<String>,
    pinned_effort: Option<String>,
    /// Codex's `thread.id` for this conversation, captured from the
    /// `thread/start` response. Persisted to `tasks.harness_session_id`
    /// for resume across app restart.
    session_id: AsyncMutex<Option<String>>,
    /// Translated event stream consumed by the host runtime.
    event_rx: AsyncMutex<Option<mpsc::UnboundedReceiver<HarnessEvent>>>,
    /// Owns the child handle (Job Object on Windows kills descendants on drop).
    child: AsyncMutex<Option<SpawnedHarnessChild>>,
    /// Tail of stderr (~64 KB) for crash-mode error enrichment.
    stderr_tail: Arc<AsyncMutex<String>>,
    /// Idle clock for the registry's reaper (plan §B.5).
    last_active: Arc<AsyncMutex<Instant>>,
    /// In-flight server-initiated approval requests:
    ///   `request_id` (string form we expose to the host) →
    ///     `(original RequestId for response, method name for shape)`.
    /// Populated by the reader task when an approval request lands;
    /// drained by `respond_to_permission` when the user clicks a button.
    pending_approvals: Arc<AsyncMutex<HashMap<String, PendingApproval>>>,
}

#[derive(Clone)]
struct PendingApproval {
    /// Original request id from the wire — must echo verbatim in the
    /// response envelope so the CLI correlates correctly.
    id: RequestId,
    /// Method name of the inbound request — drives which response shape
    /// we encode (v2 approvals use camelCase decisions; v1 used snake).
    method: String,
}

impl CodexSession {
    async fn spawn(opts: HarnessSessionOpts) -> Result<Self> {
        let program = opts
            .binary_path_override
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "codex".to_string());

        // `codex app-server` defaults to `--listen stdio://`. We pass it
        // explicitly anyway so future Codex versions that change the
        // default don't surprise us.
        let args: Vec<String> = vec![
            "app-server".into(),
            "--listen".into(),
            "stdio://".into(),
        ];

        // MCP passthrough (plan §B.12): no-op for Codex. The `app-server`
        // binary reads MCP servers from `~/.codex/config.toml`'s
        // `[mcp_servers]` section directly — there is no JSON-RPC override
        // surface on `initialize` / `thread/start` to inject them, and
        // translating Rustic's JSON config into the user's config.toml
        // would risk clobbering hand-edited entries. Users register MCP
        // servers for Codex via the `codex` CLI itself; Rustic's MCP panel
        // governs Claude Code only.
        if opts.mcp_config_path.is_some() {
            tracing::debug!(
                "codex: ignoring mcp_config_path — Codex inherits MCP servers \
                 from ~/.codex/config.toml directly"
            );
        }

        let spec = HarnessSpawnSpec {
            program,
            args,
            cwd: opts.cwd.clone(),
            env: vec![],
        };

        let mut child = SpawnedHarnessChild::spawn(spec)
            .context("failed to spawn `codex app-server`")?;

        let stdin = child
            .stdin
            .take()
            .context("codex CLI stdin missing immediately after spawn")?;
        let stdout = child
            .stdout
            .take()
            .context("codex CLI stdout missing immediately after spawn")?;
        let stderr = child
            .stderr
            .take()
            .context("codex CLI stderr missing immediately after spawn")?;

        let writer = NdjsonWriter::new(stdin);
        let pending = PendingRequests::new();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<HarnessEvent>();
        let stderr_tail = Arc::new(AsyncMutex::new(String::new()));
        let last_active: Arc<AsyncMutex<Instant>> = Arc::new(AsyncMutex::new(Instant::now()));
        let pending_approvals: Arc<AsyncMutex<HashMap<String, PendingApproval>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));

        // Reader task: parse JSON-RPC messages and dispatch.
        // - Notifications → translate to HarnessEvent → forward.
        // - Responses/Errors → resolve the matching pending request.
        // - Server-initiated Requests → for now reply with method-not-found
        //   error (approval flow is follow-up work). The CLI sees a clean
        //   refusal rather than a silent hang.
        let reader_pending = Arc::clone(&pending);
        let reader_tx = event_tx.clone();
        let reader_last_active = Arc::clone(&last_active);
        let reader_approvals = Arc::clone(&pending_approvals);
        // Hold a writer handle for replying to server-initiated requests.
        // Doing this through a clone of stdin would race with our outbound
        // request writer; instead we share the same NdjsonWriter via an
        // Arc. Rebuild that on the next refactor — for now synthesize via
        // the `pending` resolver, which doesn't need writer access.
        tokio::spawn(async move {
            let mut reader = NdjsonReader::new(stdout);
            loop {
                match reader.next_envelope().await {
                    Ok(None) => {
                        // Clean EOF; drop pending waiters so callers
                        // don't hang.
                        reader_pending.fail_all("codex app-server exited").await;
                        break;
                    }
                    Ok(Some(env)) => {
                        *reader_last_active.lock().await = Instant::now();
                        match jsonrpc::decode(&env) {
                            Some(JsonRpcMessage::Notification { method, params }) => {
                                for ev in translate_codex_notification(&method, &params) {
                                    if reader_tx.send(ev).is_err() {
                                        return;
                                    }
                                }
                            }
                            Some(JsonRpcMessage::Response { id, result }) => {
                                reader_pending.resolve(&id, Ok(result)).await;
                            }
                            Some(JsonRpcMessage::Error {
                                id,
                                code,
                                message,
                                ..
                            }) => {
                                reader_pending
                                    .resolve(
                                        &id,
                                        Err(anyhow!(format!(
                                            "codex error {code}: {message}"
                                        ))),
                                    )
                                    .await;
                            }
                            Some(JsonRpcMessage::Request { id, method, params }) => {
                                // Server-initiated request. Three approval
                                // shapes today: command exec, file change,
                                // permissions/path scope. Each gets stored
                                // by request_id so respond_to_permission
                                // can echo back with the right response.
                                if is_approval_method(&method) {
                                    let request_id_str = id.to_string();
                                    reader_approvals.lock().await.insert(
                                        request_id_str.clone(),
                                        PendingApproval {
                                            id: id.clone(),
                                            method: method.clone(),
                                        },
                                    );
                                    let (tool_name, item_id) =
                                        approval_display(&method, &params);
                                    let _ = reader_tx.send(
                                        HarnessEvent::PermissionRequest {
                                            request_id: request_id_str,
                                            tool_use_id: item_id,
                                            tool_name,
                                            input: params,
                                        },
                                    );
                                } else {
                                    tracing::warn!(
                                        method = %method,
                                        request_id = %id,
                                        "codex: unhandled server-initiated request"
                                    );
                                    let _ = reader_tx.send(HarnessEvent::Error {
                                        message: format!(
                                            "Codex sent unhandled request `{method}` — turn \
                                             may stall. Open an issue with the method name."
                                        ),
                                    });
                                }
                            }
                            None => {
                                tracing::debug!(
                                    "codex: unrecognised envelope shape: {}",
                                    env
                                );
                            }
                        }
                    }
                    Err(e) => {
                        let _ = reader_tx.send(HarnessEvent::Error {
                            message: format!("codex stream parse error: {e:#}"),
                        });
                        // Single bad line shouldn't kill the loop.
                    }
                }
            }
        });

        // Stderr drain — matches the Claude Code session shape.
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

        // Capture the picker's model/effort so every subsequent
        // `turn/start` can pass them as per-turn overrides — this is the
        // documented path on `TurnStartParams` (`model`, `effort`) and
        // makes mid-conversation switches actually take effect.
        let pinned_model = opts
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let pinned_effort = opts
            .thinking_effort
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "off")
            .map(String::from);

        let session = Self {
            writer,
            pending,
            session_id: AsyncMutex::new(None),
            event_rx: AsyncMutex::new(Some(event_rx)),
            child: AsyncMutex::new(Some(child)),
            stderr_tail,
            last_active,
            pending_approvals,
            pinned_model,
            pinned_effort,
        };

        // Initialize handshake. Codex's `initialize` params look like
        // `{ clientInfo: { name, version } }` per the schema; we keep
        // it minimal — the CLI fills in defaults for everything we omit.
        // If this errors, the spawn fails cleanly with a clear message.
        let init_params = json!({
            "clientInfo": {
                "name": "rustic",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        jsonrpc::call(&session.pending, &session.writer, "initialize", init_params)
            .await
            .context("codex `initialize` request failed")?;

        // thread/start (or thread/resume) to bring up a conversation.
        // Capture the returned `thread.id` as our session_id and emit
        // SessionReady so the host runtime can persist it.
        //
        // Model selection (when the picker handed us one) flows through
        // `thread/start.model`. Resumed threads keep whatever model they
        // were started with — the CLI doesn't accept a model swap on
        // `thread/resume`, so we don't pass it there.
        let result = if let Some(resume_id) = opts.resume_session_id.clone() {
            jsonrpc::call(
                &session.pending,
                &session.writer,
                "thread/resume",
                json!({ "threadId": resume_id }),
            )
            .await
            .context("codex `thread/resume` failed")?
        } else {
            // ThreadStartParams field name is `sandbox`, not `sandboxMode`
            // (we used to send the wrong key, which Codex silently ignored).
            // Keep model on thread/start as the *initial* setting — the
            // per-turn override path in `send_user_message` re-asserts it
            // every turn so the picker stays authoritative.
            let mut params = json!({
                "cwd": opts.cwd.to_string_lossy(),
                "sandbox": sandbox_mode_for(opts.permission_mode),
                "approvalPolicy": approval_policy_for(opts.permission_mode),
            });
            if let Some(model) = opts.model.as_deref() {
                let trimmed = model.trim();
                if !trimmed.is_empty() {
                    params
                        .as_object_mut()
                        .expect("json! produced an object literal")
                        .insert("model".into(), json!(trimmed));
                }
            }
            jsonrpc::call(
                &session.pending,
                &session.writer,
                "thread/start",
                params,
            )
            .await
            .context("codex `thread/start` failed")?
        };

        if let Some(thread_id) = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(String::from)
        {
            *session.session_id.lock().await = Some(thread_id.clone());
            // Push SessionReady through the event channel so the host
            // persists it the same way Claude Code's `system:init` does.
            let _ = event_tx.send(HarnessEvent::SessionReady {
                session_id: thread_id,
                // P0.8: Codex's `thread/start` response doesn't surface the
                // model or auth mode here. The host runtime falls back to
                // the user-picked model (passed in via prep.model) and emits
                // no auth tag for Codex; if Codex later adds these fields,
                // populate them here too.
                model: None,
                auth_mode: None,
            });
        } else {
            tracing::warn!(
                response = %result,
                "codex: thread/{} response had no thread.id",
                if opts.resume_session_id.is_some() { "resume" } else { "start" }
            );
        }

        Ok(session)
    }
}

#[async_trait]
impl HarnessSession for CodexSession {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
    }

    async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    async fn send_user_message(&self, text: String, images: Vec<HarnessImage>) -> Result<()> {
        *self.last_active.lock().await = Instant::now();

        // Codex's `turn/start` params take an `input: UserInput[]` array.
        // Per the v2 UserInput schema (docs/codex-schema/v2/...), each
        // element is `{type: "text" | "image" | "localImage", ...}`. We
        // ship base64 images as `image` entries with a data: URL — that's
        // how the schema expects inline image bytes. Confirm against a
        // real CLI version when wiring image support.
        let mut input: Vec<Value> = Vec::with_capacity(1 + images.len());
        if !text.is_empty() {
            input.push(json!({ "type": "text", "text": text }));
        }
        for img in images {
            // data: URL form. The Codex schema's ImageUserInput takes a
            // bare `url` field; data URLs are the standard inline form.
            let url = format!("data:{};base64,{}", img.media_type, img.data);
            input.push(json!({ "type": "image", "url": url }));
        }

        let session_id = self
            .session_id
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow!("turn/start before thread/start completed"))?;

        // `turn/start` is a JSON-RPC **request** per the schema (a
        // `TurnStartResponse` is paired with `TurnStartParams`) — we used to
        // send it as a notification (no `id`), and Codex silently dropped
        // those: the chat would flip to Running and never advance because
        // the CLI never actually started a turn. Now we attach an `id` so
        // Codex processes it.
        //
        // We deliberately don't `await` the response: the response carries
        // the new turn's metadata, but everything the chat UI needs flows
        // through the `item/*` and `turn/*` notifications the reader task
        // already translates. The eventual response gets resolved through
        // `pending` and dropped harmlessly when no waiter is registered
        // (we don't allocate a pending entry here).
        let mut params = json!({
            "threadId": session_id,
            "input": input,
        });
        // Per-turn model + effort overrides — `TurnStartParams` accepts both.
        // Forwarding every turn means a mid-conversation model/effort switch
        // in the picker actually drives Codex.
        if let Some(model) = self.pinned_model.as_deref() {
            params
                .as_object_mut()
                .expect("json! produced an object literal")
                .insert("model".into(), json!(model));
        }
        if let Some(effort) = self.pinned_effort.as_deref() {
            params
                .as_object_mut()
                .expect("json! produced an object literal")
                .insert("effort".into(), json!(effort));
        }

        let envelope = json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "turn/start",
            "params": params,
        });
        self.writer.write(&envelope).await
    }

    async fn respond_to_permission(
        &self,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<()> {
        *self.last_active.lock().await = Instant::now();

        let pending = {
            let mut g = self.pending_approvals.lock().await;
            g.remove(&request_id)
                .ok_or_else(|| anyhow!("no pending Codex approval for request_id {request_id}"))?
        };

        let result = encode_decision_payload(&pending.method, decision);
        let envelope = encode_response(&pending.id, result);
        self.writer.write(&envelope).await
    }

    async fn respond_to_question(&self, request_id: String, answer: String) -> Result<()> {
        // Codex uses the same applyPatch/commandExecution approval flow
        // for prompts; pure free-form questions ride on `toolRequestUserInput`
        // (per the schema). Wiring lands with approval flow.
        let _ = (request_id, answer);
        Err(anyhow!(
            "Codex question flow isn't wired yet — see plan §B.10 follow-up."
        ))
    }

    async fn interrupt(&self) -> Result<()> {
        let session_id = self.session_id.lock().await.clone();
        let envelope = json!({
            "jsonrpc": "2.0",
            "method": "turn/interrupt",
            "params": session_id.map(|tid| json!({ "threadId": tid })).unwrap_or(json!({})),
        });
        self.writer.write(&envelope).await
    }

    async fn shutdown(&self) -> Result<()> {
        // Drop all pending waiters first so any in-flight `call` returns.
        self.pending.fail_all("codex session shutdown").await;
        let mut g = self.child.lock().await;
        if let Some(mut child) = g.take() {
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

/// V2 server-initiated request methods that route to the host's
/// permission-card UI. Anything else is a non-approval request (auth
/// refresh, MCP elicitation, free-form question, etc.) and gets handled
/// elsewhere or falls through to the "unhandled" warning path.
fn is_approval_method(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
    )
}

/// Pull the `(tool_name, item_id)` to surface on the approval card from
/// each method's params. `tool_name` drives icon + label; `item_id` (when
/// present) lets the UI link the prompt back to the in-progress tool card.
fn approval_display(method: &str, params: &Value) -> (String, Option<String>) {
    let item_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .map(String::from);
    let tool_name = match method {
        "item/commandExecution/requestApproval" => "Bash",
        "item/fileChange/requestApproval" => "Edit",
        "item/permissions/requestApproval" => "Permissions",
        _ => "Tool",
    };
    (tool_name.to_string(), item_id)
}

/// Encode the `result` payload of an approval response per the v2
/// approval-decision schema. All three v2 approval methods
/// (CommandExecutionRequestApprovalResponse, FileChangeRequestApprovalResponse,
/// PermissionsRequestApprovalResponse) share the same camelCase decision
/// enum: `accept | acceptForSession | decline | cancel`. Plan §5.1 gives
/// us three buttons; we map Deny → "decline" (the agent gets an error and
/// continues) rather than "cancel" (which signals the user wants to abort
/// the whole turn).
fn encode_decision_payload(_method: &str, decision: PermissionDecision) -> Value {
    let decision_str = match decision {
        PermissionDecision::Accept => "accept",
        PermissionDecision::AcceptForSession => "acceptForSession",
        PermissionDecision::Deny => "decline",
    };
    json!({ "decision": decision_str })
}

/// Map Rustic's harness permission mode onto Codex's `sandboxMode` field
/// per plan §5. Codex distinguishes the three sandbox tiers (`read-only`,
/// `workspace-write`, `danger-full-access`) cleanly.
fn sandbox_mode_for(mode: HarnessPermissionMode) -> &'static str {
    match mode {
        HarnessPermissionMode::ReadOnly => "read-only",
        HarnessPermissionMode::Supervised | HarnessPermissionMode::AcceptEdits => {
            "workspace-write"
        }
        HarnessPermissionMode::BypassPermissions => "danger-full-access",
    }
}

/// Map Rustic's harness permission mode onto Codex's `approvalPolicy`.
/// `untrusted` (= ask for anything not on Codex's safe list) is the
/// closest match for our Supervised mode; `never` for full-auto modes.
fn approval_policy_for(mode: HarnessPermissionMode) -> &'static str {
    match mode {
        HarnessPermissionMode::ReadOnly | HarnessPermissionMode::Supervised => "untrusted",
        HarnessPermissionMode::AcceptEdits => "on-request",
        HarnessPermissionMode::BypassPermissions => "never",
    }
}

/// One-shot helper: spawn a transient `codex app-server`, run the
/// `initialize` → `model/list` JSON-RPC handshake, and return the model
/// identifiers Codex advertises. Used by the AI Settings panel to populate
/// the Codex model picker dynamically (so we don't have to ship a hardcoded
/// list that goes stale).
///
/// The child process is killed on drop (Job Object on Windows handles
/// descendants); a hard upper bound of `total_timeout` guards against a
/// hung CLI keeping the helper task alive.
///
/// Errors are bubbled up verbatim so the Tauri command can surface them
/// in the UI ("Codex CLI not found", "not signed in", etc.) — we deliberately
/// don't swallow to an empty list, otherwise the panel can't tell the
/// difference between "Codex has zero models" (impossible) and "we couldn't
/// reach the CLI".
pub async fn list_codex_models(
    binary_path_override: Option<std::path::PathBuf>,
    total_timeout: std::time::Duration,
) -> Result<Vec<String>> {
    use tokio::time::timeout;

    let program = binary_path_override
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "codex".to_string());

    let spec = HarnessSpawnSpec {
        program,
        args: vec!["app-server".into(), "--listen".into(), "stdio://".into()],
        // The model list is global to the user's Codex install — pick any
        // existing dir as cwd so the child has somewhere to live. The
        // OS temp dir is always present and writable.
        cwd: std::env::temp_dir(),
        env: vec![],
    };

    let mut child = SpawnedHarnessChild::spawn(spec)
        .context("failed to spawn `codex app-server` for model list")?;

    let stdin = child
        .stdin
        .take()
        .context("codex stdin missing immediately after spawn")?;
    let stdout = child
        .stdout
        .take()
        .context("codex stdout missing immediately after spawn")?;

    let writer = NdjsonWriter::new(stdin);
    let pending = PendingRequests::new();

    // Reader task — drives `pending.resolve` so `jsonrpc::call` can await
    // responses. We don't care about notifications here (model/list is
    // request/response only).
    let reader_pending = Arc::clone(&pending);
    let reader_handle = tokio::spawn(async move {
        let mut reader = NdjsonReader::new(stdout);
        loop {
            match reader.next_envelope().await {
                Ok(None) => {
                    reader_pending
                        .fail_all("codex app-server exited before responding")
                        .await;
                    break;
                }
                Ok(Some(env)) => match jsonrpc::decode(&env) {
                    Some(JsonRpcMessage::Response { id, result }) => {
                        reader_pending.resolve(&id, Ok(result)).await;
                    }
                    Some(JsonRpcMessage::Error {
                        id,
                        code,
                        message,
                        ..
                    }) => {
                        reader_pending
                            .resolve(&id, Err(anyhow!(format!("codex error {code}: {message}"))))
                            .await;
                    }
                    // Notifications and server-initiated requests are
                    // irrelevant for the model-list handshake; drop them.
                    _ => {}
                },
                Err(_) => break,
            }
        }
    });

    // Race the whole handshake against the caller-provided timeout so a
    // hung CLI can't leak the child indefinitely. If we trip the timeout,
    // the child is killed when we drop it below.
    let work = async {
        // Initialize with the same minimal clientInfo `CodexSession`
        // sends — keeps the wire shape consistent across the codebase.
        jsonrpc::call(
            &pending,
            &writer,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "rustic",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await
        .context("codex `initialize` failed during model list")?;

        // model/list with the default page size — Codex returns its full
        // catalogue in one page today. If pagination ever matters, follow
        // `nextCursor` here; for now one call is sufficient.
        let response = jsonrpc::call(
            &pending,
            &writer,
            "model/list",
            json!({}),
        )
        .await
        .context("codex `model/list` failed")?;

        let data = response
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("codex model/list response missing `data` array"))?;

        // Skip `hidden: true` entries — those are models Codex deliberately
        // hides from the default picker (deprecated, internal, etc.).
        // Prefer the `id` field as the canonical identifier; fall back to
        // `model` if a future schema renames things.
        let mut ids: Vec<String> = Vec::with_capacity(data.len());
        for item in data {
            if item.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }
            let id = item
                .get("id")
                .or_else(|| item.get("model"))
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(id) = id {
                ids.push(id);
            }
        }
        Ok::<Vec<String>, anyhow::Error>(ids)
    };

    let result = match timeout(total_timeout, work).await {
        Ok(r) => r,
        Err(_) => Err(anyhow!(
            "codex model list timed out after {:?}",
            total_timeout
        )),
    };

    // Best-effort tear-down — `kill().await` waits for the OS to reap.
    // Drop alone would also kill (Job Object kill-on-drop), but awaiting
    // means the next call doesn't race with a still-exiting process.
    let _ = child.kill().await;
    reader_handle.abort();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_method_recognition() {
        assert!(is_approval_method("item/commandExecution/requestApproval"));
        assert!(is_approval_method("item/fileChange/requestApproval"));
        assert!(is_approval_method("item/permissions/requestApproval"));
        assert!(!is_approval_method("item/tool/call"));
        assert!(!is_approval_method("item/tool/requestUserInput"));
        assert!(!is_approval_method("turn/started"));
    }

    #[test]
    fn approval_display_extracts_item_id_and_tool_name() {
        let (name, id) = approval_display(
            "item/commandExecution/requestApproval",
            &json!({ "itemId": "ce_42", "command": "ls" }),
        );
        assert_eq!(name, "Bash");
        assert_eq!(id.as_deref(), Some("ce_42"));

        let (name2, _) =
            approval_display("item/fileChange/requestApproval", &json!({}));
        assert_eq!(name2, "Edit");

        let (name3, _) =
            approval_display("item/permissions/requestApproval", &json!({}));
        assert_eq!(name3, "Permissions");
    }

    #[test]
    fn decision_payload_uses_camelcase_v2_strings() {
        let cases = [
            (PermissionDecision::Accept, "accept"),
            (PermissionDecision::AcceptForSession, "acceptForSession"),
            (PermissionDecision::Deny, "decline"),
        ];
        for (dec, expected) in cases {
            let payload =
                encode_decision_payload("item/commandExecution/requestApproval", dec);
            assert_eq!(payload["decision"], expected);
        }
    }

    #[test]
    fn sandbox_mode_mapping_covers_all_modes() {
        assert_eq!(sandbox_mode_for(HarnessPermissionMode::ReadOnly), "read-only");
        assert_eq!(
            sandbox_mode_for(HarnessPermissionMode::Supervised),
            "workspace-write"
        );
        assert_eq!(
            sandbox_mode_for(HarnessPermissionMode::AcceptEdits),
            "workspace-write"
        );
        assert_eq!(
            sandbox_mode_for(HarnessPermissionMode::BypassPermissions),
            "danger-full-access"
        );
    }

    #[test]
    fn approval_policy_mapping_covers_all_modes() {
        assert_eq!(
            approval_policy_for(HarnessPermissionMode::ReadOnly),
            "untrusted"
        );
        assert_eq!(
            approval_policy_for(HarnessPermissionMode::Supervised),
            "untrusted"
        );
        assert_eq!(
            approval_policy_for(HarnessPermissionMode::AcceptEdits),
            "on-request"
        );
        assert_eq!(
            approval_policy_for(HarnessPermissionMode::BypassPermissions),
            "never"
        );
    }
}
