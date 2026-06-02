//! Native FreeBuff (codebuff) provider.
//!
//! Talks directly to `codebuff.com` using the token from the local `freebuff`
//! CLI login — no external proxy process. The codebuff chat endpoint is
//! OpenAI-shaped, so this reuses [`super::openai`]'s message conversion and SSE
//! stream parser; the FreeBuff-specific parts are the `codebuff_metadata` body
//! block and the run/session state machine in [`manager`].

mod client;
mod credentials;
mod manager;
mod models;

use super::openai;
use super::{AiProvider, AiResponse, Message, ModelInfo, ProviderConfig, StreamCallback, ToolDef};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use client::{FbError, FbResult};
use manager::SessionRunManager;
use serde_json::json;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub use credentials::{detect, read_account, DetectInfo};
pub use models::{seed_models, DEFAULT_MODEL};

/// Validate a token cheaply — the account subscription endpoint returns 200 for
/// a live token and 401 for a revoked one, and (unlike a session create) spends
/// no model quota. Used by the settings UI to flag dead tokens in the pool.
pub async fn validate_token(token: &str) -> bool {
    let client = reqwest::Client::new();
    match client
        .get("https://www.codebuff.com/api/user/subscription")
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
    {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

/// After a 401 the token rests for this long before we try again.
const TOKEN_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Run/session re-init retries on a recoverable upstream failure.
const MAX_ATTEMPTS: u32 = 3;

/// Process-global session/run state, shared by every `FreeBuffProvider`
/// instance so a run + waiting-room session survives across chat calls (a fresh
/// `FreeBuffProvider` is constructed per request at the dispatch site).
fn shared_manager() -> Arc<SessionRunManager> {
    static MANAGER: OnceLock<Arc<SessionRunManager>> = OnceLock::new();
    MANAGER
        .get_or_init(|| Arc::new(SessionRunManager::new()))
        .clone()
}

/// Extra FreeBuff tokens beyond the live CLI `default` login, supplied by the
/// settings UI (one per pooled account). Merged with the CLI default by
/// `all_tokens` so the pool keeps working when an account hits its daily quota.
fn pool_tokens_store() -> &'static std::sync::Mutex<Vec<String>> {
    static POOL: OnceLock<std::sync::Mutex<Vec<String>>> = OnceLock::new();
    POOL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Replace the pooled extra-token list (called from the Tauri settings layer at
/// startup and whenever the user edits the token list).
pub fn set_pool_tokens(tokens: Vec<String>) {
    let cleaned: Vec<String> = tokens
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    *pool_tokens_store().lock().unwrap() = cleaned;
}

/// The ordered, deduped token pool: the live CLI `default` token first (so the
/// currently-logged-in account leads), then every settings-provided token.
fn all_tokens() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(t) = credentials::read_token() {
        let t = t.trim().to_string();
        if !t.is_empty() {
            out.push(t);
        }
    }
    for t in pool_tokens_store().lock().unwrap().iter() {
        if !out.iter().any(|x| x == t) {
            out.push(t.clone());
        }
    }
    out
}

/// Live model ids from a session probe; falls back to the seed list. Used by
/// the `fetch_ai_models` command.
pub async fn probe_models() -> Result<Vec<String>> {
    let token = credentials::read_token()?;
    let manager = shared_manager();
    let ids = manager
        .probe_models(&token, DEFAULT_MODEL)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;
    Ok(ids)
}

/// Model ids captured from the current live session, if any (no request spent).
/// `fetch_ai_models` prefers this over a quota-consuming probe.
pub async fn current_session_models() -> Option<Vec<String>> {
    shared_manager().current_session_models().await
}

pub struct FreeBuffProvider {
    manager: Arc<SessionRunManager>,
}

impl FreeBuffProvider {
    pub fn new() -> Self {
        Self { manager: shared_manager() }
    }
}

impl Default for FreeBuffProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// 13-char base36 client id, matching the JS `Math.random().toString(36).substring(2,15)`
/// shape the codebuff backend expects. Sourced from a v4 UUID's entropy.
fn client_session_id() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = uuid::Uuid::new_v4().as_u128();
    let mut out = [0u8; 13];
    for slot in out.iter_mut() {
        *slot = ALPHABET[(n % 36) as usize];
        n /= 36;
    }
    String::from_utf8(out.to_vec()).unwrap_or_else(|_| "0000000000000".to_string())
}

/// Map a non-2xx chat response to a recovery action.
fn classify(status: reqwest::StatusCode, body: &str) -> FbError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return FbError::Unauthorized;
    }
    let lower = body.to_lowercase();
    if status.as_u16() == 400
        && (lower.contains("runid not found") || lower.contains("runid not running"))
    {
        return FbError::RunInvalid(body.to_string());
    }
    if status == reqwest::StatusCode::CONFLICT || lower.contains("model_locked") {
        return FbError::ModelLocked;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || lower.contains("rate_limited") {
        return FbError::RateLimited { retry_after: client::parse_retry_after(body) };
    }
    const SESSION_MARKERS: &[&str] = &[
        "waiting_room_required",
        "waiting_room_queued",
        "session_superseded",
        "session_expired",
        "session_model_mismatch",
        "freebuff_update_required",
    ];
    if SESSION_MARKERS.iter().any(|m| lower.contains(m)) {
        return FbError::SessionInvalid(body.to_string());
    }
    FbError::Other(format!("{status}: {body}"))
}

/// Build the codebuff `/chat/completions` body: OpenAI shape plus
/// `codebuff_metadata`. Reuses the OpenAI message conversion.
fn build_body(
    messages: &[Message],
    tools: &[ToolDef],
    config: &ProviderConfig,
    run_id: &str,
    instance_id: &str,
) -> serde_json::Value {
    let mut api_messages = openai::convert_messages(messages);
    if let Some(sys) = &config.system_prompt {
        let has_system = api_messages
            .first()
            .map(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
            .unwrap_or(false);
        if !has_system {
            api_messages.insert(0, json!({ "role": "system", "content": sys }));
        }
    }

    let mut body = json!({
        "model": config.model,
        "max_tokens": config.max_tokens,
        "messages": api_messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if config.supports_temperature {
        body["temperature"] = json!(config.temperature);
    }
    // codebuff's chat endpoint is OpenAI-shaped but accepts ONLY `function`
    // tools: it rejects the builtin server-tool form `{"type":"web_search"}`
    // with "unknown variant `web_search`, expected `function`" (verified live).
    // So `web_search` is passed through as an ordinary function tool, exactly
    // like every other tool — the model calls it and Rustic runs the search
    // client-side via `web_tools` (Tavily/Brave), feeding results back.
    // (`web_fetch` works the same way.) Nothing is dropped.
    let oai_tools: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    if !oai_tools.is_empty() {
        body["tools"] = json!(oai_tools);
    }

    let mut meta = serde_json::Map::new();
    meta.insert("run_id".into(), json!(run_id));
    meta.insert("cost_mode".into(), json!("free"));
    meta.insert("client_id".into(), json!(client_session_id()));
    if !instance_id.is_empty() {
        meta.insert("freebuff_instance_id".into(), json!(instance_id));
    }
    body["codebuff_metadata"] = serde_json::Value::Object(meta);

    body
}

#[async_trait]
impl AiProvider for FreeBuffProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse> {
        let tokens = all_tokens();
        if tokens.is_empty() {
            return Err(anyhow!("FreeBuff is not logged in — run `freebuff login`"));
        }
        let model = config.model.clone();

        // Enough iterations to fail over through every token AND absorb a couple
        // of recoverable session/run resets on the one that ends up serving us.
        let attempts = tokens.len() as u32 + MAX_ATTEMPTS;
        let mut last_err = String::new();
        for _ in 0..attempts {
            let (token, run_id, instance_id) = self
                .manager
                .acquire(&tokens, &model)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;

            let body = build_body(&messages, &tools, config, &run_id, &instance_id);

            let resp: FbResult<reqwest::Response> = self.manager.client().chat(&token, &body).await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    self.manager.release(&token).await;
                    return Err(anyhow!(e.to_string()));
                }
            };

            let status = resp.status();
            if status.is_success() {
                let result =
                    openai::parse_completions_sse_stream(resp, stream_cb.clone(), config.cancel_token.clone())
                        .await;
                self.manager.release(&token).await;
                return result;
            }

            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(
                target: "freebuff",
                "chat failed: status={status} body={}",
                text.chars().take(400).collect::<String>()
            );
            self.manager.release(&token).await;
            match classify(status, &text) {
                FbError::Unauthorized => {
                    // Cool down THIS token and fail over to the next one rather
                    // than aborting — another pooled account may still be good.
                    self.manager.cooldown_token(&token, TOKEN_COOLDOWN).await;
                    last_err = FbError::Unauthorized.to_string();
                }
                FbError::RateLimited { retry_after } => {
                    // This account hit its daily quota. Park it and fail over to
                    // the next token — the whole point of the pool.
                    let dur = retry_after.unwrap_or(TOKEN_COOLDOWN);
                    self.manager.cooldown_token(&token, dur).await;
                    last_err = "token hit its daily rate limit".into();
                }
                FbError::Disabled => return Err(anyhow!(FbError::Disabled.to_string())),
                FbError::SessionInvalid(m) => {
                    self.manager.invalidate_session().await;
                    last_err = m;
                }
                FbError::RunInvalid(m) => {
                    self.manager.invalidate_run().await;
                    last_err = m;
                }
                FbError::ModelLocked => {
                    // The model is bound at BOTH the session and the run level:
                    // a switch needs a fresh session *and* a fresh run, or the
                    // retry just reuses the same run and hits the same lock every
                    // attempt. Dropping only the session (the old behaviour) is
                    // what left users stuck on "session locked to another model".
                    self.manager.invalidate_session().await;
                    self.manager.invalidate_run().await;
                    last_err = "session locked to another model".into();
                }
                FbError::Other(m) => return Err(anyhow!("FreeBuff upstream error: {m}")),
            }
        }

        Err(anyhow!("FreeBuff request failed after {attempts} attempts: {last_err}"))
    }

    fn name(&self) -> &str {
        "FreeBuff"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        seed_models()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_session_id_is_13_base36_chars() {
        let id = client_session_id();
        assert_eq!(id.len(), 13);
        assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        // Two draws should differ (entropy sanity check).
        assert_ne!(client_session_id(), client_session_id());
    }

    #[test]
    fn classify_maps_upstream_failures() {
        use reqwest::StatusCode;
        assert!(matches!(classify(StatusCode::UNAUTHORIZED, ""), FbError::Unauthorized));
        assert!(matches!(
            classify(StatusCode::BAD_REQUEST, "runId not found"),
            FbError::RunInvalid(_)
        ));
        assert!(matches!(classify(StatusCode::CONFLICT, ""), FbError::ModelLocked));
        assert!(matches!(
            classify(StatusCode::BAD_REQUEST, r#"{"error":"model_locked"}"#),
            FbError::ModelLocked
        ));
        assert!(matches!(
            classify(StatusCode::BAD_REQUEST, "session_expired"),
            FbError::SessionInvalid(_)
        ));
        assert!(matches!(
            classify(StatusCode::INTERNAL_SERVER_ERROR, "boom"),
            FbError::Other(_)
        ));
    }

    #[test]
    fn build_body_carries_codebuff_metadata() {
        let config = base_config();
        let body = build_body(&[], &[], &config, "run-1", "inst-1");
        let meta = &body["codebuff_metadata"];
        assert_eq!(meta["run_id"], "run-1");
        assert_eq!(meta["cost_mode"], "free");
        assert_eq!(meta["freebuff_instance_id"], "inst-1");
        assert_eq!(meta["client_id"].as_str().unwrap().len(), 13);
        // System prompt is injected as the first message.
        assert_eq!(body["messages"][0]["role"], "system");
        // Empty instance id omits the field entirely.
        let body2 = build_body(&[], &[], &config, "run-2", "");
        assert!(body2["codebuff_metadata"].get("freebuff_instance_id").is_none());
    }

    #[test]
    fn web_search_and_web_fetch_passed_as_function_tools() {
        // codebuff rejects the builtin `{"type":"web_search"}` server-tool form
        // ("unknown variant `web_search`, expected `function`", verified live),
        // so `web_search` is passed through as an ordinary function tool — the
        // model calls it and Rustic runs the search client-side. `web_fetch`
        // is handled the same way. Neither is dropped, and the builtin form is
        // never emitted.
        let mut config = base_config();
        config.web_search_enabled = true;
        let tools = vec![
            ToolDef { name: "web_search".into(), description: "x".into(), parameters: json!({}) },
            ToolDef { name: "web_fetch".into(), description: "y".into(), parameters: json!({}) },
        ];
        let body = build_body(&[], &tools, &config, "r", "i");
        let arr = body["tools"].as_array().unwrap();
        // No builtin server-tool form anywhere.
        assert!(!arr.iter().any(|t| t.get("type") == Some(&json!("web_search"))));
        // Both tools present as `function` entries.
        assert!(arr.iter().any(|t| t["type"] == "function" && t["function"]["name"] == "web_search"));
        assert!(arr.iter().any(|t| t["type"] == "function" && t["function"]["name"] == "web_fetch"));
    }

    fn base_config() -> ProviderConfig {
        ProviderConfig {
            api_key: String::new(),
            model: "deepseek/deepseek-v4-pro".into(),
            max_tokens: 4096,
            temperature: 0.7,
            base_url: None,
            system_prompt: Some("you are helpful".into()),
            thinking_budget: 0,
            context_window: 0,
            web_search_enabled: false,
            web_fetch_enabled: false,
            supports_temperature: true,
            supports_reasoning_effort: true,
            supports_adaptive_thinking: false,
            cancel_token: None,
            custom_input_cost: None,
            custom_output_cost: None,
            custom_cache_read_cost: None,
            custom_cache_write_cost: None,
            allowed_providers: None,
        }
    }
}
