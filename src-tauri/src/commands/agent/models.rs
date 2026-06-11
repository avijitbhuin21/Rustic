//! AI model-list fetching for the provider settings UI.
//!
//! Per-provider `/v1/models` calls live here, behind a 5-minute in-memory
//! cache keyed by (provider_type, key_hash, base_url) so flipping between
//! providers in the settings panel doesn't re-hit the upstream API every
//! time. The cache is process-local and dies on app restart.

use crate::state::AppState;
use crate::sync_ext::MutexExt;
use tauri::State;

// Per-provider list endpoints. Results are cached in-memory for MODEL_CACHE_TTL
// to avoid hammering the provider every time the UI refreshes the dropdown.
// The selected model id is persisted by the frontend; the fetched list itself
// is treated as live data.
const MODEL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

type ModelCacheKey = (String, u64, String, bool); // (provider, api_key hash, base_url, include_all)
type ModelCacheEntry = (Vec<String>, std::time::Instant);

static MODEL_CACHE: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<ModelCacheKey, ModelCacheEntry>>,
> = std::sync::OnceLock::new();

fn model_cache() -> &'static tokio::sync::Mutex<std::collections::HashMap<ModelCacheKey, ModelCacheEntry>> {
    MODEL_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

fn hash_key(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// On-disk cache of the FreeBuff model list (the account's real allowed models),
/// persisted so a transient probe failure / rate-limit never forces the picker
/// back to the full seed list, and so listing models never has to spend a daily
/// request once we've seen a good list. Refreshed for free from any chat's live
/// session.
fn freebuff_cache_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    crate::app_paths::app_data_dir(app)
        .ok()
        .map(|d| d.join("freebuff_models.json"))
}

fn load_freebuff_models(app: &tauri::AppHandle) -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(freebuff_cache_path(app)?).ok()?;
    let list: Vec<String> = serde_json::from_str(&raw).ok()?;
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

fn save_freebuff_models(app: &tauri::AppHandle, models: &[String]) {
    if models.is_empty() {
        return;
    }
    if let Some(path) = freebuff_cache_path(app) {
        if let Ok(json) = serde_json::to_string(models) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Fetch the live model list for a provider.
///
/// No filtering is applied: every model id the provider's API reports is
/// returned as-is. `include_all` is kept only for call-site compatibility
/// (older frontends pass it) — it no longer changes the result.
#[tauri::command]
pub async fn fetch_ai_models(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
    include_all: Option<bool>,
) -> Result<Vec<String>, String> {
    let include_all = include_all.unwrap_or(false);

    // FreeBuff is keyless — the token comes from the local CLI login, not a
    // user-entered key — so it bypasses the `__STORED__` key resolution and the
    // per-provider HTTP branches below. Resolve models from a live session probe
    // (`rateLimitsByModel`), falling back to the seed list.
    if provider_type == "FreeBuff" {
        let cache_key: ModelCacheKey =
            ("FreeBuff".to_string(), hash_key(""), String::new(), include_all);
        if !force_refresh.unwrap_or(false) {
            let cache = model_cache().lock().await;
            if let Some((models, fetched_at)) = cache.get(&cache_key) {
                if fetched_at.elapsed() < MODEL_CACHE_TTL {
                    return Ok(models.clone());
                }
            }
        }
        // Source the list cheaply, in priority order, to avoid spending one of
        // the tier's few daily requests just to populate a dropdown:
        //   1. A live session's `rateLimitsByModel` — a chat already paid for it.
        //   2. The on-disk cache from a prior good fetch.
        //   3. Only if neither exists: a probe (which DOES spend a request).
        // On any failure we use the last-known-good list, else just the default
        // model — never the full seed list, which lists models this tier can't
        // actually use.
        let mut models: Vec<String> =
            if let Some(live) = rustic_agent::provider::freebuff::current_session_models().await {
                save_freebuff_models(&app, &live);
                live
            } else if let Some(disk) = load_freebuff_models(&app) {
                disk
            } else {
                match rustic_agent::provider::freebuff::probe_models().await {
                    Ok(list) if !list.is_empty() => {
                        save_freebuff_models(&app, &list);
                        list
                    }
                    // Safe fallback: the limited-tier seed set (Flash + MiMo),
                    // not the default model alone, so MiMo is still selectable
                    // before any successful session has cached the real list.
                    _ => rustic_agent::provider::freebuff::seed_models()
                        .into_iter()
                        .map(|m| m.id)
                        .collect(),
                }
            };
        models.sort();
        models.dedup();
        let mut cache = model_cache().lock().await;
        cache.insert(cache_key, (models.clone(), std::time::Instant::now()));
        return Ok(models);
    }

    // If the webview passed the sentinel from `get_ai_config`, look up the
    // real key from the in-memory ai_config (hydrated from the keychain at
    // startup). The webview never holds the raw secret in this flow.
    let api_key = if api_key == "__STORED__" {
        let agent = state.agent.lock_safe();
        let pt = match provider_type.as_str() {
            "Claude" => Some(rustic_agent::ProviderType::Claude),
            "OpenAi" => Some(rustic_agent::ProviderType::OpenAi),
            "Gemini" => Some(rustic_agent::ProviderType::Gemini),
            "Compatible" => Some(rustic_agent::ProviderType::Compatible),
            "OpenRouter" => Some(rustic_agent::ProviderType::OpenRouter),
            _ => None,
        };
        match pt {
            Some(pt) => {
                // Compatible providers all share `ProviderType::Compatible`, so a
                // bare `provider_type == pt` match returns whichever one happens
                // to be first in the list — sending e.g. provider A's key to
                // provider B's host, which 401s. The webview passes the target
                // `base_url`, so disambiguate on it for Compatible. Fall back to
                // the first keyed entry of this type when no base_url is given
                // (legacy callers / single-key providers) so behaviour is
                // unchanged for the non-Compatible providers.
                let want = base_url.as_deref().map(|u| u.trim_end_matches('/'));
                let is_compat = pt == rustic_agent::ProviderType::Compatible;
                let providers = &agent.ai_config.providers;
                providers
                    .iter()
                    .find(|p| {
                        p.provider_type == pt
                            && !p.api_key.is_empty()
                            && (!is_compat
                                || want.map_or(true, |w| {
                                    p.base_url.as_deref().map(|b| b.trim_end_matches('/')) == Some(w)
                                }))
                    })
                    .or_else(|| {
                        providers
                            .iter()
                            .find(|p| p.provider_type == pt && !p.api_key.is_empty())
                    })
                    .map(|p| p.api_key.clone())
                    .ok_or_else(|| format!("No API key configured for {}", provider_type))?
            }
            None => return Err(format!("Unknown provider type: {}", provider_type)),
        }
    } else {
        api_key
    };

    let cache_key: ModelCacheKey = (
        provider_type.clone(),
        hash_key(&api_key),
        base_url.clone().unwrap_or_default(),
        include_all,
    );

    if !force_refresh.unwrap_or(false) {
        let cache = model_cache().lock().await;
        if let Some((models, fetched_at)) = cache.get(&cache_key) {
            if fetched_at.elapsed() < MODEL_CACHE_TTL {
                tracing::warn!(
                    "[fetch_ai_models] provider={} CACHE_HIT age={}s count={}",
                    provider_type,
                    fetched_at.elapsed().as_secs(),
                    models.len()
                );
                return Ok(models.clone());
            }
        }
    }
    tracing::warn!(
        "[fetch_ai_models] provider={} CACHE_MISS force={} base_url={:?}",
        provider_type,
        force_refresh.unwrap_or(false),
        base_url
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let models = match provider_type.as_str() {
        "Claude" => {
            let res = client
                .get("https://api.anthropic.com/v1/models")
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .query(&[("limit", "1000")])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                tracing::warn!("[fetch_ai_models] Claude HTTP error status={} body={}", status, body);
                return Err(format!("HTTP {}: {}", status, body));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            tracing::warn!(
                "[fetch_ai_models] Claude returned {} models",
                models.len()
            );
            models
        }
        "OpenAi" => {
            let res = client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(format!("HTTP {}: {}", status, body));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        "Gemini" => {
            let res = client
                .get("https://generativelanguage.googleapis.com/v1beta/models")
                .query(&[("key", api_key.as_str()), ("pageSize", "200")])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(format!("HTTP {}: {}", status, body));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let mut models: Vec<String> = data["models"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["name"].as_str().map(|s| s.replace("models/", "")))
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        "OpenRouter" => {
            let base = "https://openrouter.ai/api/v1";
            let url = format!("{}/models", base);
            let res = client
                .get(&url)
                .bearer_auth(&api_key)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(format!("OpenRouter /v1/models returned {status}: {body}"));
            }
            #[derive(serde::Deserialize)]
            struct ModelList { data: Vec<ModelEntry> }
            #[derive(serde::Deserialize)]
            struct ModelEntry { id: String }
            let list: ModelList = res.json().await.map_err(|e| e.to_string())?;
            let mut models: Vec<String> = list.data.into_iter().map(|m| m.id).collect();
            models.sort();
            models
        }
        "Compatible" => {
            let raw = base_url.clone().unwrap_or_default();
            let base = raw
                .trim_end_matches('/')
                .trim_end_matches("/chat/completions")
                .trim_end_matches("/completions")
                .trim_end_matches('/')
                .to_string();
            let url = format!("{}/models", base);
            let res = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| format!("Request to {} failed: {}", url, e))?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(format!("HTTP {} from {}: {}", status, url, body));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            // Collect model entries from whichever shape the provider uses:
            //   1. {"data": [...]}          — standard OpenAI format
            //   2. {"models": [...]}        — Ollama and some others
            //   3. [...]                    — flat array at the root (some custom APIs)
            // We merge all three so an empty "data" array doesn't shadow a
            // populated "models" array (the previous `or_else` short-circuited
            // on an empty-but-present "data" field and silently dropped the
            // "models" entries).
            let empty = vec![];
            let from_data   = data["data"].as_array().unwrap_or(&empty);
            let from_models = data["models"].as_array().unwrap_or(&empty);
            let from_root   = data.as_array().unwrap_or(&empty);
            let entries: Vec<&serde_json::Value> = if !from_data.is_empty() || !from_models.is_empty() {
                from_data.iter().chain(from_models.iter()).collect()
            } else {
                from_root.iter().collect()
            };
            let mut seen = std::collections::HashSet::new();
            let mut models: Vec<String> = entries
                .into_iter()
                .filter_map(|m| {
                    m["id"]
                        .as_str()
                        .or_else(|| m["name"].as_str())
                        .map(|s| s.to_string())
                })
                // No model filtering — the user should see every model the
                // provider offers (same policy as all the other providers
                // now). Only dedup survives.
                .filter(|id| seen.insert(id.clone()))
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        _ => return Err(format!("Unknown provider type: {}", provider_type)),
    };

    // Cache the fresh list.
    {
        let mut cache = model_cache().lock().await;
        cache.insert(cache_key, (models.clone(), std::time::Instant::now()));
    }

    Ok(models)
}

/// Owned mirror of `rustic_agent::model_registry::ModelSpec`, exposed to the
/// frontend so the Register-model modal can offer built-in models (Anthropic,
/// OpenAI, Gemini) as templates alongside any user-saved custom models.
#[derive(serde::Serialize, Clone)]
pub struct KnownModelOut {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub max_output_tokens: u32,
    pub context_window: u32,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
}

#[tauri::command]
pub fn list_known_models() -> Vec<KnownModelOut> {
    rustic_agent::model_registry::KNOWN_MODELS
        .iter()
        .map(|m| KnownModelOut {
            id: m.id.to_string(),
            name: m.name.to_string(),
            provider: m.provider.to_string(),
            max_output_tokens: m.max_output_tokens,
            context_window: m.context_window,
            input_cost_per_m: m.input_cost_per_m,
            output_cost_per_m: m.output_cost_per_m,
            cache_read_cost_per_m: m.cache_read_cost_per_m,
            cache_write_cost_per_m: m.cache_write_cost_per_m,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// OpenRouter enrichment
//
// OpenRouter is the one provider whose full catalogue (pricing, context, output
// limits, capabilities) is machine-readable from its public API, so we don't
// have to hand-maintain those entries in the static registry or make the user
// type them into the Register-model modal. Two commands back this:
//
//   fetch_openrouter_model_specs  — per-model specs for the whole catalogue,
//       used to auto-register OpenRouter models (accurate cost/context, no
//       "Setup" gate) and to feed send-time cost estimates.
//   fetch_openrouter_providers    — per-provider stats for ONE model (speed,
//       TTFT, pricing, uptime, icon) for the view-only provider panel.
//
// Pricing in the API is a string of USD-per-token; we convert to USD-per-1M to
// match the registry's convention. The speed/latency numbers come from an
// undocumented `/api/frontend/stats` endpoint (the official `/endpoints` route
// returns them as null), so that enrichment is strictly best-effort.
// ---------------------------------------------------------------------------

/// Parse one `pricing.<key>` field (USD per token, as a string) into USD per 1M.
///
/// OpenRouter uses `"-1"` as a sentinel for "price varies by routed model"
/// (e.g. the `openrouter/auto` meta-router reports `{"prompt":"-1","completion":"-1"}`).
/// A negative per-token price would otherwise propagate into the UI as a
/// nonsensical negative cost estimate, so any sub-zero value is clamped to 0.0
/// ("unknown / free") here.
fn price_per_m(pricing: &serde_json::Value, key: &str) -> f64 {
    pricing
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|per_token| *per_token >= 0.0)
        .map(|per_token| per_token * 1_000_000.0)
        .unwrap_or(0.0)
}

/// Per-model spec derived from OpenRouter's `/api/v1/models`. Mirrors the fields
/// our registry / Register-model modal care about, plus the two capability flags
/// we can infer from `supported_parameters`.
#[derive(serde::Serialize, Clone)]
pub struct OpenRouterModelSpec {
    pub id: String,
    pub name: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
    pub supports_temperature: bool,
    pub supports_reasoning_effort: bool,
}

/// The subset of an OpenRouter model spec the send path needs so cost, context
/// window, and max-output stay accurate for models that aren't in the static
/// `KNOWN_MODELS` registry. Cached process-globally by `fetch_openrouter_model_specs`.
#[derive(Clone)]
pub struct OpenRouterCost {
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
    pub context_window: u32,
    pub max_output_tokens: u32,
}

// Process-global, populated whenever `fetch_openrouter_model_specs` runs (the
// model picker calls it when an OpenRouter group is shown). The send path reads
// it via `openrouter_cost` to avoid the Sonnet-class fallback for cheap models.
static OPENROUTER_COSTS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, OpenRouterCost>>,
> = std::sync::OnceLock::new();

fn openrouter_costs() -> &'static std::sync::RwLock<std::collections::HashMap<String, OpenRouterCost>> {
    OPENROUTER_COSTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Look up cached OpenRouter pricing for a model id. Returns `None` until the
/// catalogue has been fetched at least once this session.
pub fn openrouter_cost(model_id: &str) -> Option<OpenRouterCost> {
    openrouter_costs().read().ok()?.get(model_id).cloned()
}

// Full-catalogue spec cache (the `/models` payload is large; refetching on every
// popover open is wasteful). 5-minute TTL like the model-list cache above.
static OR_SPEC_CACHE: std::sync::OnceLock<
    tokio::sync::Mutex<Option<(Vec<OpenRouterModelSpec>, std::time::Instant)>>,
> = std::sync::OnceLock::new();

fn or_spec_cache() -> &'static tokio::sync::Mutex<Option<(Vec<OpenRouterModelSpec>, std::time::Instant)>> {
    OR_SPEC_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Fetch per-model specs for OpenRouter's entire catalogue.
///
/// Populates the process-global cost cache as a side effect so cost estimates
/// for OpenRouter models stay accurate at send time. The `/models` endpoint is
/// public, so no API key is required.
#[tauri::command]
pub async fn fetch_openrouter_model_specs(
    force_refresh: Option<bool>,
) -> Result<Vec<OpenRouterModelSpec>, String> {
    if !force_refresh.unwrap_or(false) {
        let cache = or_spec_cache().lock().await;
        if let Some((specs, fetched_at)) = cache.as_ref() {
            if fetched_at.elapsed() < MODEL_CACHE_TTL {
                return Ok(specs.clone());
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default();

    let res = client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("OpenRouter /v1/models returned {status}: {body}"));
    }
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let empty = vec![];
    let entries = json["data"].as_array().unwrap_or(&empty);

    let mut specs: Vec<OpenRouterModelSpec> = Vec::with_capacity(entries.len());
    for m in entries {
        let id = match m["id"].as_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let pricing = &m["pricing"];
        let params: Vec<&str> = m["supported_parameters"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // Cap context_window at 2M to prevent absurdly large values from meta-models.
        // Most models have 128K-1M context; 2M is a reasonable upper bound.
        let context_window = m["context_length"]
            .as_u64()
            .map(|v| std::cmp::min(v, 2_000_000))
            .unwrap_or(0) as u32;
        // `top_provider.max_completion_tokens` can be null. Fall back to a
        // sensible default (16K) instead of the full context window, which
        // would leave no space for input tokens on large-context models.
        // Most models output 4k-16k tokens max; 16K is a safe upper bound.
        // Cap at 128K to prevent the API from returning nonsensical values
        // (some meta-models like openrouter/auto report 2M, which would
        // leave zero space for input and cause condense loops).
        let max_output_tokens = m["top_provider"]["max_completion_tokens"]
            .as_u64()
            .map(|v| std::cmp::min(v, 131_072))
            .unwrap_or(16_384) as u32;

        let spec = OpenRouterModelSpec {
            id: id.clone(),
            name: m["name"].as_str().unwrap_or(&id).to_string(),
            context_window,
            max_output_tokens,
            input_cost_per_m: price_per_m(pricing, "prompt"),
            output_cost_per_m: price_per_m(pricing, "completion"),
            cache_read_cost_per_m: price_per_m(pricing, "input_cache_read"),
            cache_write_cost_per_m: price_per_m(pricing, "input_cache_write"),
            supports_temperature: params.contains(&"temperature"),
            supports_reasoning_effort: params.contains(&"reasoning"),
        };
        specs.push(spec);
    }

    // Refresh the cost cache wholesale so stale entries don't linger.
    if let Ok(mut costs) = openrouter_costs().write() {
        costs.clear();
        for s in &specs {
            costs.insert(
                s.id.clone(),
                OpenRouterCost {
                    input_cost_per_m: s.input_cost_per_m,
                    output_cost_per_m: s.output_cost_per_m,
                    cache_read_cost_per_m: s.cache_read_cost_per_m,
                    cache_write_cost_per_m: s.cache_write_cost_per_m,
                    context_window: s.context_window,
                    max_output_tokens: s.max_output_tokens,
                },
            );
        }
    }

    specs.sort_by(|a, b| a.id.cmp(&b.id));
    {
        let mut cache = or_spec_cache().lock().await;
        *cache = Some((specs.clone(), std::time::Instant::now()));
    }
    Ok(specs)
}

/// Per-provider stats for a single OpenRouter model, for the view-only provider
/// panel. `provider_name` is the join key between the two sources.
#[derive(serde::Serialize, Clone, Default)]
pub struct OpenRouterProvider {
    pub provider_name: String,
    /// Lowercase provider slug used for routing (`provider.only`). Derived from
    /// the official endpoint `tag` ("deepinfra/fp4" -> "deepinfra").
    pub provider_slug: String,
    /// Favicon-style icon URL (best-effort, from the frontend stats endpoint).
    pub icon_url: Option<String>,
    /// e.g. "fp8", "fp4". `None` when the provider reports "unknown".
    pub quantization: Option<String>,
    pub context_length: u32,
    pub max_completion_tokens: Option<u32>,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
    /// Uptime % over the last 30 minutes (official, reliable).
    pub uptime_30m: Option<f64>,
    /// Median output throughput in tokens/sec (best-effort, frontend stats).
    pub throughput_tps: Option<f64>,
    /// Median time-to-first-token in ms (best-effort, frontend stats).
    pub latency_ms: Option<f64>,
    pub supported_parameters: Vec<String>,
}

// Per-model provider-stats cache, 5-minute TTL.
static OR_PROVIDER_CACHE: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<String, (Vec<OpenRouterProvider>, std::time::Instant)>>,
> = std::sync::OnceLock::new();

fn or_provider_cache() -> &'static tokio::sync::Mutex<std::collections::HashMap<String, (Vec<OpenRouterProvider>, std::time::Instant)>> {
    OR_PROVIDER_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Fetch the providers serving a given OpenRouter model, with pricing/uptime
/// (official) enriched with speed/TTFT/icon (best-effort, unofficial endpoint).
#[tauri::command]
pub async fn fetch_openrouter_providers(
    model_id: String,
    force_refresh: Option<bool>,
) -> Result<Vec<OpenRouterProvider>, String> {
    if !force_refresh.unwrap_or(false) {
        let cache = or_provider_cache().lock().await;
        if let Some((providers, fetched_at)) = cache.get(&model_id) {
            if fetched_at.elapsed() < MODEL_CACHE_TTL {
                return Ok(providers.clone());
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    // --- Source 1 (official, reliable): pricing, context, quantization, uptime.
    let url = format!(
        "https://openrouter.ai/api/v1/models/{}/endpoints",
        model_id
    );
    let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("OpenRouter endpoints returned {status}: {body}"));
    }
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let empty = vec![];
    let endpoints = json["data"]["endpoints"].as_array().unwrap_or(&empty);

    // The canonical permaslug (needed for the stats endpoint) isn't a top-level
    // field, but each endpoint's `name` is "Provider | author/slug-date" — the
    // part after " | " is exactly that slug, identical across all endpoints.
    let mut permaslug: Option<String> = None;
    let mut providers: Vec<OpenRouterProvider> = Vec::with_capacity(endpoints.len());
    for ep in endpoints {
        if permaslug.is_none() {
            if let Some((_, slug)) = ep["name"].as_str().and_then(|n| n.split_once(" | ")) {
                permaslug = Some(slug.trim().to_string());
            }
        }
        let pricing = &ep["pricing"];
        let provider_name = ep["provider_name"].as_str().unwrap_or("").to_string();
        // `tag` is like "deepinfra/fp4" or "streamlake" — the segment before
        // the first '/' is the routing slug. Fall back to a lowercased name.
        let provider_slug = ep["tag"]
            .as_str()
            .map(|t| t.split('/').next().unwrap_or(t).to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider_name.to_lowercase().replace(' ', "-"));
        providers.push(OpenRouterProvider {
            provider_name,
            provider_slug,
            icon_url: None,
            quantization: ep["quantization"]
                .as_str()
                .filter(|s| *s != "unknown" && !s.is_empty())
                .map(|s| s.to_string()),
            context_length: ep["context_length"].as_u64().unwrap_or(0) as u32,
            max_completion_tokens: ep["max_completion_tokens"].as_u64().map(|n| n as u32),
            input_cost_per_m: price_per_m(pricing, "prompt"),
            output_cost_per_m: price_per_m(pricing, "completion"),
            cache_read_cost_per_m: price_per_m(pricing, "input_cache_read"),
            cache_write_cost_per_m: price_per_m(pricing, "input_cache_write"),
            uptime_30m: ep["uptime_last_30m"].as_f64(),
            throughput_tps: ep["throughput_last_30m"].as_f64(),
            latency_ms: ep["latency_last_30m"].as_f64(),
            supported_parameters: ep["supported_parameters"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        });
    }

    // --- Source 2 (unofficial, best-effort): throughput, TTFT, provider icon.
    // Any failure here is swallowed — we still return the official data.
    if let Some(slug) = permaslug {
        let furl = format!(
            "https://openrouter.ai/api/frontend/stats/endpoint?permaslug={}&variant=standard",
            slug
        );
        if let Ok(fres) = client.get(&furl).send().await {
            if fres.status().is_success() {
                if let Ok(fjson) = fres.json::<serde_json::Value>().await {
                    if let Some(arr) = fjson["data"].as_array() {
                        for item in arr {
                            let pname = item["provider_name"].as_str().unwrap_or("");
                            if let Some(p) =
                                providers.iter_mut().find(|p| p.provider_name == pname)
                            {
                                let stats = &item["stats"];
                                if p.throughput_tps.is_none() {
                                    p.throughput_tps = stats["p50_throughput"].as_f64();
                                }
                                if p.latency_ms.is_none() {
                                    p.latency_ms = stats["p50_latency"].as_f64();
                                }
                                p.icon_url = item["provider_info"]["icon"]["url"]
                                    .as_str()
                                    .map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Fastest first (providers with no speed data sink to the bottom), then by name.
    providers.sort_by(|a, b| {
        b.throughput_tps
            .unwrap_or(0.0)
            .partial_cmp(&a.throughput_tps.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.provider_name.cmp(&b.provider_name))
    });

    {
        let mut cache = or_provider_cache().lock().await;
        cache.insert(model_id, (providers.clone(), std::time::Instant::now()));
    }
    Ok(providers)
}
