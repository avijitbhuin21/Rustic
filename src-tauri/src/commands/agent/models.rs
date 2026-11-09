//! AI model-list fetching for the provider settings UI.
//!
//! Per-provider `/v1/models` calls live here, behind a 5-minute in-memory
//! cache keyed by (provider_type, key_hash, base_url) so flipping between
//! providers in the settings panel doesn't re-hit the upstream API every
//! time. The cache is process-local and dies on app restart.

use crate::state::AppState;
use tauri::State;

/// Fetch the live model list for a provider.
///
/// No filtering is applied: every model id the provider's API reports is
/// returned as-is. `include_all` is kept only for call-site compatibility
/// (older frontends pass it) — it no longer changes the result.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn fetch_ai_models(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
    include_all: Option<bool>,
    capability: Option<String>,
) -> Result<Vec<String>, String> {
    let freebuff_cache = crate::app_paths::app_data_dir(&app)
        .ok()
        .map(|d| rustic_app::host_ops::models::freebuff_cache_path(&d));
    rustic_app::host_ops::models::fetch_ai_models(
        &state,
        freebuff_cache,
        provider_type,
        api_key,
        base_url,
        force_refresh,
        include_all,
        capability,
    )
    .await
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
// OpenRouter enrichment (shared body in rustic_app::host_ops::openrouter)
// ---------------------------------------------------------------------------

pub use rustic_app::host_ops::openrouter::{
    openrouter_cost, OpenRouterModelSpec, OpenRouterProvider,
};

/// Per-model specs for the whole OpenRouter catalogue (cached).
#[tauri::command]
pub async fn fetch_openrouter_model_specs(
    force_refresh: Option<bool>,
) -> Result<Vec<OpenRouterModelSpec>, String> {
    rustic_app::host_ops::openrouter::fetch_openrouter_model_specs(force_refresh).await
}

/// Per-provider endpoint stats for one OpenRouter model (cached).
#[tauri::command]
pub async fn fetch_openrouter_providers(
    model_id: String,
    force_refresh: Option<bool>,
) -> Result<Vec<OpenRouterProvider>, String> {
    rustic_app::host_ops::openrouter::fetch_openrouter_providers(model_id, force_refresh).await
}
