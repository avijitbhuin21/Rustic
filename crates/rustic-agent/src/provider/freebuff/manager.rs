//! Stateful run + free-session orchestration shared across all FreeBuff chat
//! requests. Mirrors the token-pool / run-manager logic of the Go proxy, scoped
//! to the single `default` account in `credentials.json`.
//!
//! Concurrency: all state transitions hold an async [`tokio::Mutex`]; the lock
//! is held across the network calls inside `acquire` (StartRun / session create
//! / waiting-room polling) so concurrent chats serialize through one consistent
//! run + session. Streaming itself happens *after* `acquire` returns and the
//! lock is released, so multiple responses can stream at once (tracked by
//! `inflight`).

use super::client::{FbClient, FbError, FbResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Runs rotate after this age; matches the Go proxy's 6h default.
const RUN_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);
/// Waiting-room poll interval is clamped to this range.
const POLL_MIN: Duration = Duration::from_secs(1);
const POLL_MAX: Duration = Duration::from_secs(5);
/// Give up waiting in the queue after this long so a chat fails cleanly instead
/// of hanging the UI indefinitely.
const MAX_QUEUE_WAIT: Duration = Duration::from_secs(90);
/// Fallback cooldown for a rate-limited token when the 429 body carries no
/// `retryAfterMs` — long enough to stop hammering, short enough to recheck.
const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
struct RunState {
    run_id: String,
    started_at: Instant,
    step_count: u32,
}

#[derive(Clone)]
struct SessionState {
    instance_id: String,
    model: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Model ids from this session's `rateLimitsByModel` — the account's real
    /// allowed list, captured for free whenever any chat creates a session, so
    /// the picker never has to spend a daily request just to list models.
    models: Vec<String>,
}

impl SessionState {
    fn is_expired(&self) -> bool {
        match self.expires_at {
            // Treat as expired a few seconds early to avoid racing the boundary.
            Some(exp) => chrono::Utc::now() + chrono::Duration::seconds(5) >= exp,
            None => false,
        }
    }
}

#[derive(Default)]
struct Inner {
    run: Option<RunState>,
    /// Rotated-out runs awaiting FINISH once `inflight` drains to zero.
    draining: Vec<RunState>,
    session: Option<SessionState>,
    inflight: u32,
    /// The token the current run/session belong to. A single session is active
    /// at a time (codebuff allows one per token); switching tokens resets it.
    active_token: Option<String>,
    /// Per-token cooldown deadlines. A token rate-limited (429) or auth-rejected
    /// (401) rests here until its deadline; `acquire` skips it and fails over to
    /// the next token in the pool — that's what gives multi-account 24/7 cover.
    cooldowns: HashMap<String, Instant>,
}

pub struct SessionRunManager {
    client: Arc<FbClient>,
    inner: Mutex<Inner>,
}

impl SessionRunManager {
    pub fn new() -> Self {
        Self {
            client: Arc::new(FbClient::new()),
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn client(&self) -> Arc<FbClient> {
        self.client.clone()
    }

    /// Ensure a valid run + a session bound to `model` on SOME token from the
    /// pool, then register one in-flight request. Returns `(token, run_id,
    /// freebuff_instance_id)` — the chosen token, so the caller chats and
    /// releases on it. Prefers the currently-bound token (reuses its session),
    /// then fails over to the next token, skipping any still cooling down from a
    /// rate-limit / auth failure. That round-robin failover is what gives a
    /// multi-account pool continuous coverage past one account's daily quota.
    pub async fn acquire(
        &self,
        tokens: &[String],
        model: &str,
    ) -> FbResult<(String, String, String)> {
        if tokens.is_empty() {
            return Err(FbError::Other("no FreeBuff tokens configured".into()));
        }
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        inner.cooldowns.retain(|_, until| *until > now);

        // Candidate order: the currently-bound token first (so we reuse its live
        // session instead of churning), then the rest in pool order; skip any on
        // cooldown.
        let mut candidates: Vec<String> = Vec::new();
        if let Some(active) = inner.active_token.clone() {
            if tokens.iter().any(|t| t == &active) && !inner.cooldowns.contains_key(&active) {
                candidates.push(active);
            }
        }
        for t in tokens {
            if inner.cooldowns.contains_key(t) {
                continue;
            }
            if !candidates.iter().any(|c| c == t) {
                candidates.push(t.clone());
            }
        }

        if candidates.is_empty() {
            let secs = inner
                .cooldowns
                .values()
                .min()
                .map(|u| u.saturating_duration_since(now).as_secs())
                .unwrap_or(0);
            return Err(FbError::Other(format!(
                "all {} FreeBuff token(s) are rate-limited; next retry in ~{}s (resets at pacific midnight)",
                tokens.len(),
                secs
            )));
        }

        let total = candidates.len();
        let mut last_err: Option<FbError> = None;
        for (i, token) in candidates.into_iter().enumerate() {
            match self.acquire_on_token(&mut inner, &token, model).await {
                Ok((run_id, instance_id)) => return Ok((token, run_id, instance_id)),
                Err(FbError::RateLimited { retry_after }) => {
                    let dur = retry_after.unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN);
                    tracing::warn!(target: "freebuff", "token {}/{} rate-limited; cooling down {}s, failing over", i + 1, total, dur.as_secs());
                    inner.cooldowns.insert(token.clone(), Instant::now() + dur);
                    last_err = Some(FbError::RateLimited { retry_after });
                }
                Err(FbError::Unauthorized) => {
                    tracing::warn!(target: "freebuff", "token {}/{} rejected (401); cooling down, failing over", i + 1, total);
                    inner
                        .cooldowns
                        .insert(token.clone(), Instant::now() + DEFAULT_RATE_LIMIT_COOLDOWN);
                    last_err = Some(FbError::Unauthorized);
                }
                Err(e) => {
                    tracing::warn!(target: "freebuff", "token {}/{} acquire failed: {e}; trying next", i + 1, total);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err
            .unwrap_or_else(|| FbError::Other("FreeBuff: no token could acquire a session".into())))
    }

    /// Run-rotation + session-ensure for one specific token. Switching tokens
    /// resets the (per-token) run/session, ending the old token's session
    /// best-effort. Assumes the caller holds the lock.
    async fn acquire_on_token(
        &self,
        inner: &mut Inner,
        token: &str,
        model: &str,
    ) -> FbResult<(String, String)> {
        // Switching to a different token: the cached run/session belong to the
        // old token and can't be reused. End the old token's session (frees its
        // one-session slot) and start fresh on the new token.
        if inner.active_token.as_deref() != Some(token) {
            if let Some(old) = inner.active_token.clone() {
                let _ = self.client.end_session(&old).await;
            }
            inner.run = None;
            inner.session = None;
            inner.draining.clear(); // old-token runs can't be FINISHed by the new token; abandon
            inner.active_token = Some(token.to_string());
        }

        // Rotate an aged run out for draining.
        if let Some(run) = &inner.run {
            if run.started_at.elapsed() >= RUN_MAX_AGE {
                let old = inner.run.take().unwrap();
                inner.draining.push(old);
            }
        }

        // A model switch invalidates the current run too: codebuff binds the
        // model to the run on first use, so reusing the old run for a different
        // model makes chat return `model_locked`. Rotate the stale run out.
        let model_changed = inner
            .session
            .as_ref()
            .map(|s| s.model != model)
            .unwrap_or(false);
        if model_changed {
            if let Some(old) = inner.run.take() {
                inner.draining.push(old);
            }
        }

        tracing::info!(
            target: "freebuff",
            "acquire model={model} bound_model={:?} model_changed={model_changed} have_run={}",
            inner.session.as_ref().map(|s| s.model.as_str()),
            inner.run.is_some()
        );

        // Ensure a run.
        if inner.run.is_none() {
            let run_id = self.client.start_run(token).await?;
            inner.run = Some(RunState {
                run_id,
                started_at: Instant::now(),
                step_count: 0,
            });
        }
        let run_id = inner.run.as_ref().unwrap().run_id.clone();

        // Ensure a session bound to `model`.
        let instance_id = self.ensure_session(inner, token, model).await?;

        if let Some(run) = inner.run.as_mut() {
            run.step_count = run.step_count.saturating_add(1);
        }
        inner.inflight = inner.inflight.saturating_add(1);
        Ok((run_id, instance_id))
    }

    /// Reusable session for `model`, creating / polling as needed. Assumes the
    /// caller holds the lock.
    async fn ensure_session(
        &self,
        inner: &mut Inner,
        token: &str,
        model: &str,
    ) -> FbResult<String> {
        // Reuse a live, model-matched session.
        if let Some(s) = &inner.session {
            if s.model == model && !s.is_expired() {
                return Ok(s.instance_id.clone());
            }
            // Bound to a different model (or expired): end the token's active
            // session before rebinding.
            inner.session = None;
            let _ = self.client.end_session(token).await;
        }

        let deadline = Instant::now() + MAX_QUEUE_WAIT;
        let mut resp = self.create_session_rebinding(token, model).await?;
        loop {
            match resp.status.as_str() {
                "disabled" => return Err(FbError::Disabled),
                "queued" => {
                    if Instant::now() >= deadline || resp.instance_id.is_empty() {
                        return Err(FbError::Other(
                            "FreeBuff waiting room is busy — try again shortly".into(),
                        ));
                    }
                    let delay = Duration::from_millis(resp.estimated_wait_ms.max(0) as u64)
                        .clamp(POLL_MIN, POLL_MAX);
                    tokio::time::sleep(delay).await;
                    resp = self.client.poll_session(token, &resp.instance_id).await?;
                }
                // "active" — and any other non-queued/non-disabled status — is
                // usable; bind to whatever instance we got (possibly empty).
                _ => {
                    let expires_at = resp
                        .expires_at
                        .as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&chrono::Utc));
                    let instance_id = resp.instance_id.clone();
                    let models: Vec<String> = resp.rate_limits_by_model.keys().cloned().collect();
                    if !models.is_empty() {
                        tracing::info!(
                            target: "freebuff",
                            "session model list (rateLimitsByModel keys) = {:?}",
                            models
                        );
                    }
                    inner.session = Some(SessionState {
                        instance_id: instance_id.clone(),
                        model: model.to_string(),
                        expires_at,
                        models,
                    });
                    return Ok(instance_id);
                }
            }
        }
    }

    /// Create a session, recovering from a server-side `model_locked`: the token
    /// already holds an active session bound to a *different* model (a leftover
    /// from a previous selection or a crashed run that Rustic's cache doesn't
    /// know about). End that session and create a fresh one bound to `model`.
    async fn create_session_rebinding(
        &self,
        token: &str,
        model: &str,
    ) -> FbResult<super::client::SessionResponse> {
        match self.client.create_session(token, model).await {
            Ok(resp) => Ok(resp),
            Err(FbError::ModelLocked) => {
                let _ = self.client.end_session(token).await;
                self.client.create_session(token, model).await
            }
            Err(e) => Err(e),
        }
    }

    /// Release one in-flight request. When the last one drains, FINISH any
    /// rotated-out runs (best-effort).
    pub async fn release(&self, token: &str) {
        let draining = {
            let mut inner = self.inner.lock().await;
            inner.inflight = inner.inflight.saturating_sub(1);
            if inner.inflight == 0 && !inner.draining.is_empty() {
                std::mem::take(&mut inner.draining)
            } else {
                Vec::new()
            }
        };
        for run in draining {
            let _ = self
                .client
                .finish_run(token, &run.run_id, run.step_count)
                .await;
        }
    }

    /// Drop the current session so the next acquire recreates it.
    pub async fn invalidate_session(&self) {
        self.inner.lock().await.session = None;
    }

    /// Drop the current run so the next acquire starts a fresh one.
    pub async fn invalidate_run(&self) {
        self.inner.lock().await.run = None;
    }

    /// Put one token on cooldown (after a 429 rate-limit or 401) so `acquire`
    /// skips it and fails over to the next token. If it's the active token, drop
    /// its run/session so the next acquire rebuilds on a different token.
    pub async fn cooldown_token(&self, token: &str, dur: Duration) {
        let mut inner = self.inner.lock().await;
        inner
            .cooldowns
            .insert(token.to_string(), Instant::now() + dur);
        if inner.active_token.as_deref() == Some(token) {
            inner.run = None;
            inner.session = None;
            inner.active_token = None;
        }
    }

    /// Probe a session purely to read the live model list (`rateLimitsByModel`).
    /// Does not touch the shared run/session state. Returns model ids.
    /// Model ids captured from the current live session (no request spent).
    /// `None` when there's no session or it carried no list — callers then fall
    /// back to the on-disk cache rather than spending a daily request to probe.
    pub async fn current_session_models(&self) -> Option<Vec<String>> {
        let inner = self.inner.lock().await;
        inner
            .session
            .as_ref()
            .filter(|s| !s.models.is_empty())
            .map(|s| s.models.clone())
    }

    pub async fn probe_models(&self, token: &str, model: &str) -> FbResult<Vec<String>> {
        // Use the rebinding-aware create (end_session + retry on model_locked),
        // NOT the bare client call. A stale session left bound to another model
        // (e.g. by a failed Kimi/Pro attempt) made the bare probe fail with
        // model_locked → the command fell back to the full seed list, which is
        // exactly why the picker showed every model including ones this tier
        // can't use.
        let resp = self.create_session_rebinding(token, model).await?;
        // Dump the full rateLimitsByModel map so we can see whether the server
        // lists models the tier can't actually use (and what field, if any,
        // distinguishes a usable model from a blocked one) — that's what decides
        // how to filter the picker.
        tracing::info!(
            target: "freebuff",
            "probe_models rateLimitsByModel={}",
            serde_json::to_string(&resp.rate_limits_by_model).unwrap_or_default()
        );
        Ok(resp.rate_limits_by_model.keys().cloned().collect())
    }
}
