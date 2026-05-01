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

type ModelCacheKey = (String, u64, String); // (provider, api_key hash, base_url)
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

#[tauri::command]
pub async fn fetch_ai_models(
    state: State<'_, AppState>,
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
) -> Result<Vec<String>, String> {
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

    let client = reqwest::Client::new();

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
                    m["supportedGenerationMethods"]
                        .as_array()
                        .map(|methods| methods.contains(&generate_content))
                        .unwrap_or(false)
                })
                .filter_map(|m| m["name"].as_str().map(|s| s.replace("models/", "")))
                .filter(|id| {
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
            let mut models: Vec<String> = data["data"]
                .as_array()
                .or_else(|| data["models"].as_array())
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| {
                    m["id"]
                        .as_str()
                        .or_else(|| m["name"].as_str())
                        .map(|s| s.to_string())
                })
                .filter(|id| {
                    let id_lower = id.to_lowercase();
                    !NON_CHAT_KEYWORDS.iter().any(|kw| id_lower.contains(kw))
                })
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
