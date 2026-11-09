//! AI model-list fetching for the provider settings UI, shared by both hosts.
//!
//! Per-provider `/v1/models` calls live here, behind a 5-minute in-memory
//! cache keyed by (provider_type, key_hash, base_url) so flipping between
//! providers in the settings panel doesn't re-hit the upstream API every
//! time. The cache is process-local and dies on restart.

use std::path::{Path, PathBuf};

use crate::state::AppState;
use crate::sync_ext::MutexExt;

pub(crate) const MODEL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

type ModelCacheKey = (String, u64, String, bool); // (provider, api_key hash, base_url, include_all)
type ModelCacheEntry = (Vec<String>, std::time::Instant);

static MODEL_CACHE: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<ModelCacheKey, ModelCacheEntry>>,
> = std::sync::OnceLock::new();

/// Process-wide model-list cache.
fn model_cache(
) -> &'static tokio::sync::Mutex<std::collections::HashMap<ModelCacheKey, ModelCacheEntry>> {
    MODEL_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Hash an API key so the cache key never holds the raw secret.
fn hash_key(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Where the FreeBuff model list is persisted under a host's data dir.
pub fn freebuff_cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("freebuff_models.json")
}

/// Read the persisted FreeBuff model list (the account's real allowed models).
fn load_freebuff_models(path: Option<&Path>) -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(path?).ok()?;
    let list: Vec<String> = serde_json::from_str(&raw).ok()?;
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// Persist a known-good FreeBuff model list so a probe failure never falls back to the full seed list.
fn save_freebuff_models(path: Option<&Path>, models: &[String]) {
    if models.is_empty() {
        return;
    }
    if let (Some(path), Ok(json)) = (path, serde_json::to_string(models)) {
        let _ = std::fs::write(path, json);
    }
}

/// Fetch the model ids a provider offers (cached 5 min); `api_key == "__STORED__"` resolves the key from the in-memory AI config.
pub async fn fetch_ai_models(
    state: &AppState,
    freebuff_cache: Option<PathBuf>,
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
    include_all: Option<bool>,
) -> Result<Vec<String>, String> {
    let include_all = include_all.unwrap_or(false);

    if let Some(ref u) = base_url {
        rustic_agent::provider::validate_provider_base_url(u)
            .map_err(|e| format!("Invalid base_url: {}", e))?;
    }

    // FreeBuff is keyless — the token comes from the local CLI login, not a
    // user-entered key — so it bypasses the `__STORED__` key resolution and the
    // per-provider HTTP branches below. Resolve models from a live session probe
    // (`rateLimitsByModel`), falling back to the seed list.
    if provider_type == "FreeBuff" {
        let cache_key: ModelCacheKey = (
            "FreeBuff".to_string(),
            hash_key(""),
            String::new(),
            include_all,
        );
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
                save_freebuff_models(freebuff_cache.as_deref(), &live);
                live
            } else if let Some(disk) = load_freebuff_models(freebuff_cache.as_deref()) {
                disk
            } else {
                match rustic_agent::provider::freebuff::probe_models().await {
                    Ok(list) if !list.is_empty() => {
                        save_freebuff_models(freebuff_cache.as_deref(), &list);
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
                                || want.is_none_or(|w| {
                                    p.base_url.as_deref().map(|b| b.trim_end_matches('/'))
                                        == Some(w)
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
                tracing::debug!(
                    "[fetch_ai_models] provider={} CACHE_HIT age={}s count={}",
                    provider_type,
                    fetched_at.elapsed().as_secs(),
                    models.len()
                );
                return Ok(models.clone());
            }
        }
    }
    tracing::debug!(
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
                tracing::warn!(
                    "[fetch_ai_models] Claude HTTP error status={} body={}",
                    status,
                    body
                );
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
            tracing::warn!("[fetch_ai_models] Claude returned {} models", models.len());
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
            struct ModelList {
                data: Vec<ModelEntry>,
            }
            #[derive(serde::Deserialize)]
            struct ModelEntry {
                id: String,
            }
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
            let from_data = data["data"].as_array().unwrap_or(&empty);
            let from_models = data["models"].as_array().unwrap_or(&empty);
            let from_root = data.as_array().unwrap_or(&empty);
            let entries: Vec<&serde_json::Value> =
                if !from_data.is_empty() || !from_models.is_empty() {
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
