//! Static registry of known AI models and their capabilities.
//!
//! Used to auto-resolve `max_tokens` and provide cost estimates in the UI.
//! For unknown models (e.g. via Compatible provider), the user supplies these values manually.

use serde::{Deserialize, Serialize};

/// Per-model capabilities and pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Model ID as sent to the API (e.g. "claude-sonnet-4-6-20260401").
    pub id: &'static str,
    /// Human-readable display name.
    pub name: &'static str,
    /// Maximum output tokens the model can generate in a single response.
    pub max_output_tokens: u32,
    /// Cost per 1 million input tokens, in USD.
    pub input_cost_per_m: f64,
    /// Cost per 1 million output tokens, in USD.
    pub output_cost_per_m: f64,
    /// Provider identifier (matches ProviderType variants).
    pub provider: &'static str,
}

/// All known models across all providers.
///
/// Last updated: April 2026.
pub static KNOWN_MODELS: &[ModelSpec] = &[
    // ═══════════════════════════════════════════════════════════════════════════
    // Anthropic (Claude) — https://platform.claude.com/docs/en/about-claude/pricing
    // ═══════════════════════════════════════════════════════════════════════════

    // ── 4.6 (current flagship) ───────────────────────────────────────────────
    ModelSpec {
        id: "claude-opus-4-6-20260401",
        name: "Claude Opus 4.6",
        max_output_tokens: 128_000,
        input_cost_per_m: 5.0,
        output_cost_per_m: 25.0,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-sonnet-4-6-20260401",
        name: "Claude Sonnet 4.6",
        max_output_tokens: 64_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        provider: "Claude",
    },
    // ── 4.5 ──────────────────────────────────────────────────────────────────
    ModelSpec {
        id: "claude-sonnet-4-5-20250514",
        name: "Claude Sonnet 4.5",
        max_output_tokens: 64_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-haiku-4-5-20251001",
        name: "Claude Haiku 4.5",
        max_output_tokens: 64_000,
        input_cost_per_m: 1.0,
        output_cost_per_m: 5.0,
        provider: "Claude",
    },
    // ── 4.0 (legacy, still in API) ───────────────────────────────────────────
    ModelSpec {
        id: "claude-opus-4-20250514",
        name: "Claude Opus 4",
        max_output_tokens: 128_000,
        input_cost_per_m: 5.0,
        output_cost_per_m: 25.0,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-sonnet-4-20250514",
        name: "Claude Sonnet 4",
        max_output_tokens: 64_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        provider: "Claude",
    },

    // ═══════════════════════════════════════════════════════════════════════════
    // OpenAI — https://developers.openai.com/api/docs/pricing
    // ═══════════════════════════════════════════════════════════════════════════

    // ── GPT-5.4 family (current flagship, March 2026) ────────────────────────
    ModelSpec {
        id: "gpt-5.4-pro",
        name: "GPT-5.4 Pro",
        max_output_tokens: 128_000,
        input_cost_per_m: 30.0,
        output_cost_per_m: 180.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5.4",
        name: "GPT-5.4",
        max_output_tokens: 128_000,
        input_cost_per_m: 2.50,
        output_cost_per_m: 15.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5.4-mini",
        name: "GPT-5.4 Mini",
        max_output_tokens: 128_000,
        input_cost_per_m: 0.75,
        output_cost_per_m: 4.50,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5.4-nano",
        name: "GPT-5.4 Nano",
        max_output_tokens: 128_000,
        input_cost_per_m: 0.20,
        output_cost_per_m: 1.25,
        provider: "OpenAi",
    },

    // ── Codex models (agentic coding) ────────────────────────────────────────
    ModelSpec {
        id: "gpt-5.3-codex",
        name: "GPT-5.3 Codex",
        max_output_tokens: 128_000,
        input_cost_per_m: 1.75,
        output_cost_per_m: 14.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5-codex",
        name: "GPT-5 Codex",
        max_output_tokens: 128_000,
        input_cost_per_m: 1.25,
        output_cost_per_m: 10.0,
        provider: "OpenAi",
    },

    // ── Reasoning models ─────────────────────────────────────────────────────
    ModelSpec {
        id: "o4-mini",
        name: "o4-mini",
        max_output_tokens: 100_000,
        input_cost_per_m: 1.10,
        output_cost_per_m: 4.40,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "o3",
        name: "o3",
        max_output_tokens: 100_000,
        input_cost_per_m: 2.0,
        output_cost_per_m: 8.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "o3-mini",
        name: "o3-mini",
        max_output_tokens: 100_000,
        input_cost_per_m: 1.10,
        output_cost_per_m: 4.40,
        provider: "OpenAi",
    },

    // ── GPT-4.1 family (legacy) ──────────────────────────────────────────────
    ModelSpec {
        id: "gpt-4.1",
        name: "GPT-4.1",
        max_output_tokens: 32_768,
        input_cost_per_m: 2.0,
        output_cost_per_m: 8.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-4.1-mini",
        name: "GPT-4.1 Mini",
        max_output_tokens: 32_768,
        input_cost_per_m: 0.40,
        output_cost_per_m: 1.60,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-4.1-nano",
        name: "GPT-4.1 Nano",
        max_output_tokens: 32_768,
        input_cost_per_m: 0.10,
        output_cost_per_m: 0.40,
        provider: "OpenAi",
    },

    // ── GPT-4o family (legacy) ───────────────────────────────────────────────
    ModelSpec {
        id: "gpt-4o",
        name: "GPT-4o",
        max_output_tokens: 16_384,
        input_cost_per_m: 2.50,
        output_cost_per_m: 10.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-4o-mini",
        name: "GPT-4o Mini",
        max_output_tokens: 16_384,
        input_cost_per_m: 0.15,
        output_cost_per_m: 0.60,
        provider: "OpenAi",
    },

    // ═══════════════════════════════════════════════════════════════════════════
    // Google Gemini — https://ai.google.dev/gemini-api/docs/pricing
    // ═══════════════════════════════════════════════════════════════════════════

    // ── 3.1 family (current flagship, Feb 2026) ─────────────────────────────
    ModelSpec {
        id: "gemini-3.1-pro",
        name: "Gemini 3.1 Pro",
        max_output_tokens: 65_536,
        input_cost_per_m: 2.0,
        output_cost_per_m: 12.0,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-3.1-flash-lite",
        name: "Gemini 3.1 Flash-Lite",
        max_output_tokens: 65_536,
        input_cost_per_m: 0.25,
        output_cost_per_m: 1.50,
        provider: "Gemini",
    },

    // ── 3.0 family ───────────────────────────────────────────────────────────
    ModelSpec {
        id: "gemini-3-flash",
        name: "Gemini 3 Flash",
        max_output_tokens: 65_536,
        input_cost_per_m: 0.50,
        output_cost_per_m: 3.0,
        provider: "Gemini",
    },

    // ── 2.5 family (still widely used) ───────────────────────────────────────
    ModelSpec {
        id: "gemini-2.5-pro",
        name: "Gemini 2.5 Pro",
        max_output_tokens: 65_536,
        input_cost_per_m: 1.25,
        output_cost_per_m: 10.0,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-2.5-flash",
        name: "Gemini 2.5 Flash",
        max_output_tokens: 65_536,
        input_cost_per_m: 0.15,
        output_cost_per_m: 0.60,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-2.5-flash-lite",
        name: "Gemini 2.5 Flash-Lite",
        max_output_tokens: 65_536,
        input_cost_per_m: 0.10,
        output_cost_per_m: 0.40,
        provider: "Gemini",
    },

    // ── 2.0 (deprecated June 2026, still in API) ────────────────────────────
    ModelSpec {
        id: "gemini-2.0-flash",
        name: "Gemini 2.0 Flash",
        max_output_tokens: 8_192,
        input_cost_per_m: 0.10,
        output_cost_per_m: 0.40,
        provider: "Gemini",
    },
];

/// Look up a model by its ID. Tries exact match first, then bidirectional prefix match:
/// - Forward: model_id starts with m.id  (user gave a more specific version, e.g. with date suffix)
/// - Reverse: m.id starts with model_id  (user gave a shorter alias, e.g. "claude-opus-4-6"
///                                         matches registry entry "claude-opus-4-6-20260401")
/// Longest common prefix wins to pick the most specific match.
pub fn lookup(model_id: &str) -> Option<&'static ModelSpec> {
    // Exact match first
    if let Some(spec) = KNOWN_MODELS.iter().find(|m| m.id == model_id) {
        return Some(spec);
    }
    // Bidirectional prefix match — score by length of the shared prefix characters
    KNOWN_MODELS
        .iter()
        .filter(|m| model_id.starts_with(m.id) || m.id.starts_with(model_id))
        .max_by_key(|m| {
            // Count matching characters from the start
            m.id.chars()
                .zip(model_id.chars())
                .take_while(|(a, b)| a == b)
                .count()
        })
}

/// Get the max output tokens for a model. Returns the registry value if known,
/// or `fallback` for unknown models.
pub fn max_output_tokens(model_id: &str, fallback: u32) -> u32 {
    lookup(model_id).map(|m| m.max_output_tokens).unwrap_or(fallback)
}

/// Get all known models for a given provider (e.g. "Claude", "OpenAi", "Gemini").
pub fn models_for_provider(provider: &str) -> Vec<&'static ModelSpec> {
    KNOWN_MODELS
        .iter()
        .filter(|m| m.provider == provider)
        .collect()
}
