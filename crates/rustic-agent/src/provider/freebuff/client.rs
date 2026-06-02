//! Low-level HTTP against codebuff.com — run lifecycle, free-session lifecycle,
//! and the chat POST. Stateless: all session/run state lives in
//! [`super::manager::SessionRunManager`]. Higher layers classify failures via
//! [`FbError`].

use serde::Deserialize;
use std::collections::HashMap;

pub const BASE_URL: &str = "https://www.codebuff.com";
pub const USER_AGENT: &str = "ai-sdk/openai-compatible/1.0.25/codebuff";
/// All free-tier models route through this single agent; the per-session
/// `x-freebuff-model` binding selects the actual model server-side.
pub const AGENT_ID: &str = "base2-free";

/// Classified upstream failure. Drives the retry / cooldown / rebind policy in
/// the provider and manager.
#[derive(Debug)]
pub enum FbError {
    /// 401 — token rejected. Caller should cool the token down (~30m).
    Unauthorized,
    /// Free sessions are disabled for this token.
    Disabled,
    /// Session must be recreated (waiting_room_required, session_superseded,
    /// session_expired, session_model_mismatch, freebuff_update_required).
    SessionInvalid(String),
    /// 409 / model_locked — end the current session and rebind to the model.
    ModelLocked,
    /// Run is no longer valid (400 "runId not found" / "not running").
    RunInvalid(String),
    /// 429 — this token hit its per-model daily quota. The caller cools this
    /// token down (ideally until the server's `retryAfterMs`) and fails over to
    /// the next token in the pool. `retry_after` is the server-reported wait
    /// when parseable from the body.
    RateLimited { retry_after: Option<std::time::Duration> },
    /// Transport error, 5xx, or any unclassified failure.
    Other(String),
}

impl std::fmt::Display for FbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FbError::Unauthorized => write!(f, "FreeBuff token rejected (401) — run `freebuff login`"),
            FbError::Disabled => write!(f, "FreeBuff free sessions are disabled for this account"),
            FbError::SessionInvalid(m) => write!(f, "FreeBuff session invalid: {m}"),
            FbError::ModelLocked => write!(f, "FreeBuff session is locked to another model"),
            FbError::RunInvalid(m) => write!(f, "FreeBuff run invalid: {m}"),
            FbError::RateLimited { .. } => write!(f, "FreeBuff token hit its daily rate limit"),
            FbError::Other(m) => write!(f, "FreeBuff upstream error: {m}"),
        }
    }
}

impl std::error::Error for FbError {}

pub type FbResult<T> = std::result::Result<T, FbError>;

/// Parsed `/api/v1/freebuff/session` response. Unknown fields are ignored;
/// missing fields default so partial bodies don't fail to deserialize.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default, rename = "instanceId")]
    pub instance_id: String,
    #[serde(default, rename = "estimatedWaitMs")]
    pub estimated_wait_ms: i64,
    #[serde(default, rename = "expiresAt")]
    pub expires_at: Option<String>,
    #[serde(default, rename = "rateLimitsByModel")]
    pub rate_limits_by_model: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct StartRunResponse {
    #[serde(default, rename = "runId")]
    run_id: String,
}

pub struct FbClient {
    client: reqwest::Client,
}

impl FbClient {
    pub fn new() -> Self {
        // No overall request timeout: chat responses stream and may stay open
        // for the duration of a long generation, matching OpenAiProvider.
        Self { client: reqwest::Client::new() }
    }

    fn auth(&self, builder: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
        builder
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", USER_AGENT)
    }

    /// `POST /api/v1/agent-runs {action:START, agentId}` → runId.
    pub async fn start_run(&self, token: &str) -> FbResult<String> {
        let url = format!("{BASE_URL}/api/v1/agent-runs");
        let body = serde_json::json!({ "action": "START", "agentId": AGENT_ID });
        let resp = self
            .auth(self.client.post(&url), token)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| FbError::Other(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(FbError::Unauthorized);
        }
        if !status.is_success() {
            return Err(FbError::Other(format!("StartRun {status}: {text}")));
        }
        let parsed: StartRunResponse = serde_json::from_str(&text)
            .map_err(|e| FbError::Other(format!("StartRun bad body: {e}")))?;
        if parsed.run_id.is_empty() {
            return Err(FbError::Other("StartRun returned no runId".into()));
        }
        Ok(parsed.run_id)
    }

    /// `POST /api/v1/agent-runs {action:FINISH, …}`. Best-effort; errors ignored
    /// by callers.
    pub async fn finish_run(&self, token: &str, run_id: &str, total_steps: u32) -> FbResult<()> {
        let url = format!("{BASE_URL}/api/v1/agent-runs");
        let body = serde_json::json!({
            "action": "FINISH",
            "runId": run_id,
            "status": "completed",
            "totalSteps": total_steps,
            "directCredits": 0,
            "totalCredits": 0,
        });
        self.auth(self.client.post(&url), token)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| FbError::Other(e.to_string()))?;
        Ok(())
    }

    /// `POST /api/v1/freebuff/session` with `x-freebuff-model`. Creates or
    /// refreshes the free session and binds it to `model`.
    pub async fn create_session(&self, token: &str, model: &str) -> FbResult<SessionResponse> {
        let url = format!("{BASE_URL}/api/v1/freebuff/session");
        let resp = self
            .auth(self.client.post(&url), token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("x-freebuff-model", model)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| FbError::Other(e.to_string()))?;
        let out = self.parse_session(resp).await;
        match &out {
            Ok(s) => tracing::info!(
                target: "freebuff",
                "create_session model={model} -> ok instance={}",
                s.instance_id
            ),
            Err(e) => tracing::warn!(target: "freebuff", "create_session model={model} -> err: {e}"),
        }
        out
    }

    /// `GET /api/v1/freebuff/session` with `x-freebuff-instance-id` — poll a
    /// queued session for promotion to `active`.
    pub async fn poll_session(&self, token: &str, instance_id: &str) -> FbResult<SessionResponse> {
        let url = format!("{BASE_URL}/api/v1/freebuff/session");
        let resp = self
            .auth(self.client.get(&url), token)
            .header("Accept", "application/json")
            .header("x-freebuff-instance-id", instance_id)
            .send()
            .await
            .map_err(|e| FbError::Other(e.to_string()))?;
        self.parse_session(resp).await
    }

    /// `DELETE /api/v1/freebuff/session` — end the token's current active
    /// session. No instance id is sent: codebuff ends whatever session the token
    /// currently holds, which is exactly what we need to clear a stale lock bound
    /// to a different model before rebinding. Best-effort.
    pub async fn end_session(&self, token: &str) -> FbResult<()> {
        let url = format!("{BASE_URL}/api/v1/freebuff/session");
        let resp = self
            .auth(self.client.delete(&url), token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| FbError::Other(e.to_string()))?;
        // Log the outcome — a stale lock that won't clear shows up here as a
        // non-2xx status or an error body, which the old best-effort version
        // swallowed silently.
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::info!(
            target: "freebuff",
            "end_session -> {status} {}",
            body.chars().take(200).collect::<String>()
        );
        Ok(())
    }

    /// `POST /api/v1/chat/completions`. The body (OpenAI shape + `codebuff_metadata`)
    /// is built by the caller. Returns the raw response so the caller can stream
    /// 2xx bodies and classify non-2xx status itself.
    pub async fn chat(
        &self,
        token: &str,
        body: &serde_json::Value,
    ) -> FbResult<reqwest::Response> {
        let url = format!("{BASE_URL}/api/v1/chat/completions");
        self.auth(self.client.post(&url), token)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body)
            .send()
            .await
            .map_err(|e| FbError::Other(e.to_string()))
    }

    async fn parse_session(&self, resp: reqwest::Response) -> FbResult<SessionResponse> {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(FbError::Unauthorized);
        }
        let lower = text.to_lowercase();
        if status == reqwest::StatusCode::CONFLICT || lower.contains("model_locked") {
            return Err(FbError::ModelLocked);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || lower.contains("rate_limited") {
            return Err(FbError::RateLimited { retry_after: parse_retry_after(&text) });
        }
        if !status.is_success() {
            return Err(FbError::Other(format!("session {status}: {text}")));
        }
        let parsed: SessionResponse = serde_json::from_str(&text)
            .map_err(|e| FbError::Other(format!("session bad body: {e}")))?;
        Ok(parsed)
    }
}

/// Extract `retryAfterMs` from a FreeBuff 429 body and clamp it to a sane range
/// so a multi-hour value doesn't park a token for the rest of the day with no
/// recheck (the daily window resets at pacific midnight regardless).
pub fn parse_retry_after(body: &str) -> Option<std::time::Duration> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let ms = v.get("retryAfterMs").and_then(|x| x.as_i64()).filter(|x| *x > 0)?;
    let secs = (ms / 1000).clamp(60, 6 * 60 * 60) as u64; // 1 min … 6 h
    Some(std::time::Duration::from_secs(secs))
}
