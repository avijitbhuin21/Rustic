//! AI model-list fetching for the provider settings UI.
//!
//! Per-provider `/v1/models` calls live here, behind a 5-minute in-memory
//! cache keyed by (provider_type, key_hash, base_url) so flipping between
//! providers in the settings panel doesn't re-hit the upstream API every
//! time. The cache is process-local and dies on app restart.

use crate::state::AppState;
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

/// Shared non-text model keywords. Anything containing one of these in its id
/// is not a chat model and should be hidden from the picker.
const NON_CHAT_KEYWORDS: &[&str] = &[
    "tts", "whisper", "dall-e", "embedding", "moderation",
    "speech", "audio", "image-gen", "transcri", "realtime",
];

/// Fetch the live model list for a provider.
///
/// `include_all`: when `Some(true)`, skip the chat-only filter and return
/// every model id the provider reports. Used by the media-tool settings UI
/// (image / video / animate) where image-gen and video-gen models must be
/// selectable. Defaults to `false` to keep the existing chat picker behavior.
#[tauri::command]
pub async fn fetch_ai_models(
    state: State<'_, AppState>,
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
    include_all: Option<bool>,
) -> Result<Vec<String>, String> {
    let include_all = include_all.unwrap_or(false);
    // Harness providers don't have an API key — short-circuit before the
    // keychain lookup so the frontend doesn't have to fake a sentinel.
    if provider_type == "ClaudeCode" {
        return Ok(vec!["claude-code".to_string()]);
    }

    // If the webview passed the sentinel from `get_ai_config`, look up the
    // real key from the in-memory ai_config (hydrated from the keychain at
    // startup). The webview never holds the raw secret in this flow.
    let api_key = if api_key == "__STORED__" {
        let agent = state.agent.lock().unwrap();
        let pt = match provider_type.as_str() {
            "Claude" => Some(rustic_agent::ProviderType::Claude),
            "OpenAi" => Some(rustic_agent::ProviderType::OpenAi),
            "Gemini" => Some(rustic_agent::ProviderType::Gemini),
            "Compatible" => Some(rustic_agent::ProviderType::Compatible),
            "OpenRouter" => Some(rustic_agent::ProviderType::OpenRouter),
            "ClaudeCode" => Some(rustic_agent::ProviderType::ClaudeCode),
            _ => None,
        };
        match pt {
            Some(pt) => agent
                .ai_config
                .providers
                .iter()
                .find(|p| p.provider_type == pt && !p.api_key.is_empty())
                .map(|p| p.api_key.clone())
                .ok_or_else(|| format!("No API key configured for {}", provider_type))?,
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

            // Dump every id the API returned so we can tell real filter bugs
            // from API-side omissions.
            let raw_ids: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect();
            tracing::warn!(
                "[fetch_ai_models] Claude raw ids ({}): {:?}",
                raw_ids.len(),
                raw_ids
            );

            let mut models: Vec<String> = raw_ids
                .into_iter()
                .filter(|id| {
                    let is_claude = id.starts_with("claude-");
                    let has_tier = id.contains("haiku") || id.contains("sonnet") || id.contains("opus");
                    let keep = is_claude && has_tier;
                    if !keep {
                        tracing::warn!("[fetch_ai_models] Claude DROP id={}", id);
                    }
                    keep
                })
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            tracing::warn!(
                "[fetch_ai_models] Claude kept ({}): {:?}",
                models.len(),
                models
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
                return Err(format!("HTTP {}", res.status()));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            // Chat-capable families only: gpt-*, chatgpt-*, and reasoning o-series
            // (o1, o3, o4, o5...). Excludes embeddings, tts, whisper, etc.
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .filter(|id| {
                    if include_all { return true; }
                    let id_lower = id.to_lowercase();
                    let is_chat_family = id_lower.starts_with("gpt-")
                        || id_lower.starts_with("chatgpt-")
                        || (id_lower.starts_with('o')
                            && id_lower.chars().nth(1).map_or(false, |c| c.is_ascii_digit()));
                    is_chat_family
                        && !NON_CHAT_KEYWORDS.iter().any(|kw| id_lower.contains(kw))
                        // "search" models are retrieval helpers, not chat
                        && !id_lower.contains("search")
                })
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
                return Err(format!("HTTP {}", res.status()));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let generate_content = serde_json::json!("generateContent");
            // Any model that supports generateContent and isn't an embedding
            // surface. Includes both dated variants and -latest aliases.
            let mut models: Vec<String> = data["models"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter(|m| {
                    if include_all { return true; }
                    m["supportedGenerationMethods"]
                        .as_array()
                        .map(|methods| methods.contains(&generate_content))
                        .unwrap_or(false)
                })
                .filter_map(|m| m["name"].as_str().map(|s| s.replace("models/", "")))
                .filter(|id| {
                    if include_all { return true; }
                    let id_lower = id.to_lowercase();
                    !NON_CHAT_KEYWORDS.iter().any(|kw| id_lower.contains(kw))
                        // Gecko / aqa / text-bison-style legacy embeddings
                        && !id_lower.contains("gecko")
                        && !id_lower.contains("aqa")
                })
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
                // Compatible providers are user-configured endpoints; the user
                // knows what they connected and should see every model the
                // provider offers. We do NOT apply NON_CHAT_KEYWORDS here
                // because dated/audio/realtime variants are valid model choices
                // and the old filter was hiding them without any indication.
                // (NON_CHAT filtering still applies to the standard OpenAI
                // provider where the family-prefix filter already handles
                // chat-vs-non-chat separation.)
                .filter(|id| seen.insert(id.clone()))
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        "ClaudeCode" => {
            // Harness providers don't expose a model list — Claude Code's CLI
            // picks the model itself based on the user's subscription. We
            // surface a single placeholder so the existing model-picker UI
            // shows something selectable.
            vec!["claude-code".to_string()]
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
