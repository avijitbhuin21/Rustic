//! Fallback model list used before (or when) a live `rateLimitsByModel` probe
//! is unavailable. Mirrors the Go proxy's seed set.

use super::super::ModelInfo;

/// Seed model ids, used as the safe fallback when a live session / disk cache /
/// probe can't supply the real list. Scoped to the two models the "limited"
/// free tier actually allows — pro / kimi / minimax are rejected on this tier,
/// so listing them only produced failing selections. The real per-account list
/// (which may include more on a higher tier) still comes from a live session's
/// `rateLimitsByModel`, captured for free and cached to disk.
pub const SEED_MODEL_IDS: &[&str] = &["deepseek/deepseek-v4-flash", "mimo/mimo-v2.5"];

/// Default model selected when the FreeBuff provider is first enabled. Flash is
/// the safest choice — it is allowed on the limited free tier (where pro/kimi
/// are rejected with `session_model_mismatch`).
pub const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash";

/// Build a display name from a `vendor/model` id (e.g. `deepseek/deepseek-v4-pro`
/// → "Deepseek V4 Pro").
fn display_name(id: &str) -> String {
    let tail = id.rsplit('/').next().unwrap_or(id);
    tail.split(['-', '_', '.'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Seed models as [`ModelInfo`] (free tier → zero cost).
pub fn seed_models() -> Vec<ModelInfo> {
    SEED_MODEL_IDS.iter().map(|id| model_info(id)).collect()
}

/// Build a [`ModelInfo`] for an arbitrary FreeBuff model id.
pub fn model_info(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: display_name(id),
        max_tokens: 16384,
        input_cost_per_m: 0.0,
        output_cost_per_m: 0.0,
    }
}
