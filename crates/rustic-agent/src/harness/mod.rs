//! External agent harness layer.
//!
//! A `Harness` wraps an external agent process (Claude Code CLI, Codex) that
//! owns its own tool loop and session state. Rustic spawns the binary, streams
//! NDJSON from stdout, and translates events to `TaskEvent` for the frontend.
//! Contrast with [`crate::provider`]: providers are API clients Rustic drives;
//! harnesses drive themselves.

pub mod auth_check;
pub mod claude_code;
pub mod codex;
pub mod event_map;
pub mod event_map_codex;
pub mod jsonrpc;
pub mod process_spawn;
pub mod stream_json;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};

/// Stable identifier for a kind of harness. Used by the task runtime to
/// dispatch and by settings/UI to label provider entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    ClaudeCode,
    Codex,
}

impl HarnessKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude_code",
            HarnessKind::Codex => "codex",
        }
    }
}

/// How aggressively the harness should auto-approve tool use.
///
/// Maps to `--permission-mode` for Claude Code and to
/// `approval_policy` + `sandbox_mode` for Codex (see plan §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessPermissionMode {
    /// Read-only: never write, never run shell commands.
    ReadOnly,
    /// Prompt the user for any tool that mutates state.
    Supervised,
    /// Auto-approve safe edits inside the workspace; prompt for sensitive ops.
    AcceptEdits,
    /// Auto-approve everything, including shell commands outside the sandbox.
    BypassPermissions,
}

/// Per-call decision returned in response to a `HarnessEvent::PermissionRequest`.
///
/// The third variant `AcceptForSession` is what makes harness UX bearable:
/// without it, the user re-approves the same tool every turn. See plan §5.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Allow this single tool call; the next call of the same tool re-prompts.
    Accept,
    /// Allow this tool (and equivalent invocations) for the rest of the session.
    AcceptForSession,
    /// Reject; the harness surfaces a tool error to the agent.
    Deny,
}

/// Inline image attached to a user message. Base64-encoded payload + IANA
/// media type (`image/png`, `image/jpeg`, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessImage {
    pub media_type: String,
    pub data: String,
}

/// Options passed to `Harness::start_session`. Kept intentionally small —
/// the harness owns its own system prompt, tool set, and project context;
/// we only hand it the things the CLI flag surface needs.
#[derive(Debug, Clone)]
pub struct HarnessSessionOpts {
    /// Working directory the CLI should run in. For project tasks this is the
    /// project root; for the Global orchestrator chat it's the per-user scratch
    /// directory described in plan §13.2.
    pub cwd: PathBuf,
    /// Initial permission mode. The user can re-prompt the harness to change
    /// it later via in-chat slash commands; we don't need to track that.
    pub permission_mode: HarnessPermissionMode,
    /// If `Some`, attempt to resume an existing CLI session by ID
    /// (`claude --resume <id>` / Codex `session.restore`).
    pub resume_session_id: Option<String>,
    /// Absolute path to the binary, or `None` to use the binary name on PATH
    /// (default `claude` / `codex`). Surfaced as the user-overridable
    /// `binaryPath` setting (plan §13.1).
    pub binary_path_override: Option<PathBuf>,
    /// Optional path to a Rustic-managed MCP config file
    /// (`{ "mcpServers": { ... } }`) to pass through to the harness so the
    /// CLI inherits user-scoped MCP servers configured inside Rustic
    /// (plan §B.12). Project-scoped servers (`<project>/.mcp.json`) are
    /// auto-discovered by the CLI from `cwd` and don't need to be plumbed
    /// here. `None` (or a missing file) skips the passthrough cleanly.
    ///
    /// Only honored by the Claude Code harness — Codex reads its own MCP
    /// servers from `~/.codex/config.toml` and the JSON-RPC `initialize`
    /// surface has no override slot.
    pub mcp_config_path: Option<PathBuf>,
    /// Model identifier the user picked in the agent-config dropdown.
    /// For Claude Code, this becomes `--model <id>` on spawn (the CLI
    /// accepts both bare aliases like `sonnet` and full names). For Codex,
    /// this is forwarded as the `model` field on `thread/start`. `None`
    /// means "let the CLI use its own default" — the picker should always
    /// give us one for harness tasks, but the runtime stays robust to
    /// missing values.
    pub model: Option<String>,
    /// Thinking / reasoning-effort level the user picked in the agent
    /// config popover. Free-form lowercase string (`low`, `medium`,
    /// `high`, `xhigh`, `max`, `minimal`, `none`) so we don't have to
    /// teach this enum every supported tier per provider. Each harness
    /// validates against the CLI's own accepted set:
    ///   * Claude Code → forwarded as `--effort <level>`
    ///   * Codex       → forwarded as `config.model_reasoning_effort`
    ///                   on `thread/start`
    /// `None` means "no effort override" — the CLI uses the model's
    /// default reasoning effort, which is what most users want.
    pub thinking_effort: Option<String>,
}

/// Events streamed out of a running harness session. Maps 1:1 onto Rustic's
/// existing `TaskEvent` protocol — the task runtime is the translator.
#[derive(Debug, Clone)]
pub enum HarnessEvent {
    /// Session is alive and the CLI has reported its session ID. We persist
    /// this so we can resume after the process is reaped.
    ///
    /// P0.8: `model` and `auth_mode` are populated when the CLI emits them
    /// on the session-init envelope. We capture them here (rather than the
    /// harness_runtime caller passing them in) because the CLI is the
    /// authoritative source — see [harness/event_map.rs] `system:init`.
    /// `auth_mode` mirrors Claude Code's `apiKeySource` ("ANTHROPIC_API_KEY",
    /// "subscription", etc.) and drives the "(API)" vs "(sub estimate)" cost
    /// tag. Both are `Option<String>` so older CLIs / Codex sessions that
    /// don't emit one of these fields still work — the runtime falls back to
    /// the user-picked model and prints no tag.
    SessionReady {
        session_id: String,
        model: Option<String>,
        auth_mode: Option<String>,
    },
    /// Streaming assistant text delta.
    TextDelta { text: String },
    /// Streaming extended-thinking delta (Claude Code only emits this when
    /// the user has thinking enabled in their CLI config).
    ThinkingDelta { text: String },
    /// A tool call started. `input` is the fully-parsed JSON the harness sent.
    /// `diff_payload` is populated by `event_map` for `Edit`/`Write`/Codex
    /// equivalents so the frontend can render a real diff card (plan §6.1).
    ToolUse {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        diff_payload: Option<DiffPayload>,
    },
    /// Result of a previously-emitted `ToolUse`. `is_error` is true when the
    /// tool itself failed (file-not-found, command exited non-zero, ...).
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// The harness is asking for explicit user approval before running a tool.
    /// Respond with `HarnessSession::respond_to_permission(request_id, ...)`.
    ///
    /// `tool_use_id` is the CLI's identifier for the specific tool call this
    /// permission gates — distinct from `request_id` (the control-protocol id
    /// used to correlate the response). Both are surfaced so the UI can
    /// match the prompt to the in-progress tool card.
    PermissionRequest {
        request_id: String,
        tool_use_id: Option<String>,
        tool_name: String,
        input: serde_json::Value,
    },
    /// Token / cost / rate-limit accounting for the just-finished turn.
    ///
    /// P0.8: `cli_reported_cost_usd` carries Claude Code's own
    /// `total_cost_usd` (or Codex's equivalent) when the result envelope
    /// includes it. When present, the host should prefer it over locally
    /// recomputing from token counts × per-model rates — the CLI already
    /// summed across all the models it actually used (Opus + Haiku mix in
    /// auto-mode, etc.) and the local recompute would attribute everything
    /// to a single model.
    Usage {
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
        rate_limit: Option<RateLimitSnapshot>,
        cli_reported_cost_usd: Option<f64>,
    },
    /// The CLI finished a turn cleanly. The next user message starts a new turn.
    TurnComplete,
    /// The CLI is asking the user a question (e.g. a choice prompt). Unlike
    /// `PermissionRequest` (which gates a specific tool call), this is a
    /// free-form interactive prompt. Respond with
    /// `HarnessSession::respond_to_question(request_id, answer)`.
    UserQuestion {
        request_id: String,
        question: String,
        choices: Vec<String>,
    },
    /// P0.9 fix #8: a tool whose execution requires explicit user approval
    /// (Claude Code's `exit_plan_mode`, future approval-gated tools). The
    /// frontend renders a specialised card per `kind` instead of the
    /// generic "the agent wants to use tool X" permission row — most
    /// important case is `exit_plan_mode` where the payload carries the
    /// agent's proposed plan and we want a "Review plan / Approve / Deny"
    /// dialog instead of a one-line tool-name prompt.
    ///
    /// Responses route through `HarnessSession::respond_to_permission`
    /// (Allow → execute the tool, Deny → reject) — same wire as the
    /// existing PermissionRequest path. The host emits this variant
    /// instead of PermissionRequest when it detects an approval-gated
    /// tool by name; the CLI itself doesn't tag them differently, the
    /// classification happens in the event_map translator.
    ApprovalRequest {
        request_id: String,
        tool_use_id: Option<String>,
        /// e.g. `"exit_plan_mode"`. Frontend keys off this for the card variant.
        kind: String,
        /// Tool input verbatim. For `exit_plan_mode` this contains `{"plan": "..."}`.
        payload: serde_json::Value,
    },
    /// P0.9 fix #8: MCP elicitation prompt. An MCP server connected to the
    /// CLI is asking the user for structured input via JSON-schema. The
    /// frontend renders a schema-driven form; the user's answers route
    /// back via the same `respond_to_question` path as UserQuestion (the
    /// CLIs accept either a text answer or a serialised JSON object).
    ///
    /// `schema` is the raw JSON-schema from the elicitation envelope; if
    /// rendering it dynamically is too complex for the first pass, the
    /// frontend can fall back to a free-text dialog with the schema
    /// displayed for context.
    McpElicit {
        request_id: String,
        message: String,
        schema: serde_json::Value,
    },
    /// P0.9: catch-all for any envelope type the translator doesn't yet
    /// recognise as one of the typed variants above. Surfaced so the user
    /// gets a visible "the agent is asking something I don't know how to
    /// render — here's the raw text, type a reply" dialog rather than the
    /// CLI hanging forever on an envelope we silently dropped.
    ///
    /// `envelope_type` is the top-level `type` (Claude Code) or `method`
    /// (Codex) string — useful for the UI to format a clearer header.
    /// `summary` is a best-effort plain-text excerpt for the dialog body.
    /// `raw` carries the full envelope so the UI can pretty-print it for
    /// debugging.
    ///
    /// **No response method is wired yet** — for this catch-all path the
    /// host responds via existing `respond_to_question` if a `request_id`
    /// was extractable, otherwise the user has to abort the turn. Adding
    /// typed responses per envelope shape is follow-up work tracked
    /// alongside the per-variant dialog components.
    UnknownPrompt {
        envelope_type: String,
        request_id: Option<String>,
        summary: String,
        raw: serde_json::Value,
    },
    /// Fatal error from the harness (process crashed, schema mismatch, etc.).
    /// The session is dead after this fires.
    Error { message: String },
}

/// Diff payload attached to `ToolUse` for edit-shaped tools. Built in
/// `event_map.rs`; the frontend reuses Rustic's existing diff renderer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffPayload {
    pub file_path: String,
    /// `None` for brand-new files (renders as "+ all lines").
    pub old_content: Option<String>,
    pub new_content: String,
}

/// Rolling rate-limit window snapshot. Surfaced in the chat header pill
/// (plan §10.1). For Anthropic Pro/Max this is the 5-hour window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    pub window: String,
    pub percent_used: f32,
    pub resets_at_iso: Option<String>,
}

/// Trait implemented per CLI (Claude Code, Codex). Cheap to construct; the
/// expensive bit (spawning the process) happens lazily inside `start_session`.
#[async_trait]
pub trait Harness: Send + Sync {
    fn kind(&self) -> HarnessKind;

    /// Spawn the CLI and return a live session bound to a fresh stdin/stdout
    /// pair. Errors here are setup-time errors (binary missing, not signed in,
    /// bad cwd) — runtime errors come through `HarnessEvent::Error` instead.
    async fn start_session(
        &self,
        opts: HarnessSessionOpts,
    ) -> Result<Arc<dyn HarnessSession>>;
}

/// Live session against a running CLI process. All methods are non-blocking
/// w.r.t. the I/O loop — they just enqueue an envelope to be written.
#[async_trait]
pub trait HarnessSession: Send + Sync {
    /// The kind that produced this session. Used by the task runtime to
    /// route translation through the right `event_map` entries.
    fn kind(&self) -> HarnessKind;

    /// CLI-reported session ID, once known. `None` until `SessionReady` fires.
    /// Persisted to `tasks.harness_session_id` for resume.
    async fn session_id(&self) -> Option<String>;

    /// Append a new user turn. Multiple calls before the previous turn ends
    /// are queued by the CLI itself (plan §14).
    async fn send_user_message(&self, text: String, images: Vec<HarnessImage>) -> Result<()>;

    /// Reply to a previously-emitted `PermissionRequest`.
    async fn respond_to_permission(
        &self,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<()>;

    /// Free-form answer to a `tool_use_id`-tagged user-question prompt. Some
    /// CLIs use this for non-permission interactive prompts.
    async fn respond_to_question(&self, request_id: String, answer: String) -> Result<()>;

    /// Politely ask the CLI to abort the current turn. Falls through to a
    /// hard kill if the CLI doesn't ack within the deadline (plan §13.5).
    async fn interrupt(&self) -> Result<()>;

    /// Tear down: drain remaining events, kill the child if still alive,
    /// release any platform-specific handles (Job Object on Windows). Idempotent.
    async fn shutdown(&self) -> Result<()>;

    /// Take ownership of the event receiver. Returns `None` if already taken
    /// — only one consumer per session.
    async fn take_event_rx(&self) -> Option<mpsc::UnboundedReceiver<HarnessEvent>>;

    /// Best-effort tail of the CLI's stderr (~last 64 KB). Used by the host
    /// runtime to enrich the failure message when the child dies before the
    /// turn completes — without this the user sees a bare "Failed" status
    /// and no clue why. Implementations that don't capture stderr return
    /// an empty string.
    async fn stderr_tail(&self) -> String {
        String::new()
    }

    /// Most recent moment this session saw activity — either the user
    /// sending a message or an event arriving from the CLI. Drives the
    /// idle reaper (`HarnessRegistry::reap_idle`); harnesses that don't
    /// track activity return `Instant::now()` here so they're effectively
    /// never reaped (matches plan §8). Plan §B.5.
    async fn last_active(&self) -> Instant {
        Instant::now()
    }
}

/// Process-wide registry of live sessions, keyed by task ID.
///
/// The task runtime owns the registry; it inserts on lazy spawn, removes on
/// idle reap or task delete (plan §8). Used by the app-quit handler to drop
/// every live session so no orphan CLI processes survive the Tauri shutdown.
#[derive(Default)]
pub struct HarnessRegistry {
    sessions: Mutex<HashMap<String, Arc<dyn HarnessSession>>>,
}

impl HarnessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, task_id: String, session: Arc<dyn HarnessSession>) {
        let mut g = self.sessions.lock().await;
        g.insert(task_id, session);
    }

    pub async fn get(&self, task_id: &str) -> Option<Arc<dyn HarnessSession>> {
        let g = self.sessions.lock().await;
        g.get(task_id).cloned()
    }

    pub async fn remove(&self, task_id: &str) -> Option<Arc<dyn HarnessSession>> {
        let mut g = self.sessions.lock().await;
        g.remove(task_id)
    }

    /// Shut down every live session. Called from the Tauri close-requested
    /// handler so no CLI process outlives the app.
    pub async fn shutdown_all(&self) {
        let drained: Vec<_> = {
            let mut g = self.sessions.lock().await;
            g.drain().collect()
        };
        for (task_id, session) in drained {
            if let Err(e) = session.shutdown().await {
                tracing::warn!(task = %task_id, error = %e, "harness shutdown failed");
            }
        }
    }

    pub async fn len(&self) -> usize {
        self.sessions.lock().await.len()
    }

    /// Snapshot of currently-live task IDs in the registry. Used by the
    /// agent panel to render the live-agent counter / banner without
    /// exposing the underlying session objects (plan §B.6 / §B.14).
    pub async fn task_ids(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }

    /// Drop and shut down every session whose `last_active` is older than
    /// `threshold`. Called periodically from a background task (plan §B.5)
    /// so idle CLI processes don't sit on ~150–300 MB of Node memory each.
    /// Resume on next message-send is automatic via the persisted
    /// `harness_session_id` and `--resume <id>`.
    ///
    /// Race notes: a fresh `send_message` may insert a new session for the
    /// same task between the snapshot and the remove step. The pointer
    /// identity check on remove guards against reaping that fresh session.
    pub async fn reap_idle(&self, threshold: Duration) {
        let cutoff = match Instant::now().checked_sub(threshold) {
            Some(c) => c,
            // Process clock hasn't run long enough to subtract `threshold` —
            // nothing can be that old yet, so nothing to reap.
            None => return,
        };

        // Snapshot first so we don't hold the registry lock across each
        // session's `last_active().await` (would serialise all sessions
        // and stall fresh `send_message` calls during the check).
        let snapshot: Vec<(String, Arc<dyn HarnessSession>)> = {
            let g = self.sessions.lock().await;
            g.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect()
        };

        let mut to_reap: Vec<(String, Arc<dyn HarnessSession>)> = Vec::new();
        for (task_id, session) in snapshot {
            if session.last_active().await <= cutoff {
                to_reap.push((task_id, session));
            }
        }

        for (task_id, session) in to_reap {
            // Re-acquire the lock and verify the registry still holds the
            // same session pointer. If a fresh send_message replaced it
            // since the snapshot, leave the new session alone.
            let mut g = self.sessions.lock().await;
            let still_same = g
                .get(&task_id)
                .map(|cur| Arc::ptr_eq(cur, &session))
                .unwrap_or(false);
            if !still_same {
                continue;
            }
            g.remove(&task_id);
            drop(g);

            if let Err(e) = session.shutdown().await {
                tracing::warn!(task = %task_id, error = %e, "idle reap shutdown failed");
            } else {
                tracing::info!(task = %task_id, "reaped idle harness session");
            }
        }
    }
}
