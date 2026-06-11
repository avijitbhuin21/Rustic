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
    /// Total context window in tokens (input + output combined).
    /// The `[1m]` Claude variant (1M context) is applied dynamically on top
    /// of this at lookup time, not encoded per-entry.
    pub context_window: u32,
    /// Cost per 1 million input tokens, in USD.
    pub input_cost_per_m: f64,
    /// Cost per 1 million output tokens, in USD.
    pub output_cost_per_m: f64,
    /// Cost per 1 million cache-read (hit) input tokens, in USD.
    /// Anthropic: ~10% of input. OpenAI: 20–50% depending on family. Gemini: ~25%.
    pub cache_read_cost_per_m: f64,
    /// Cost per 1 million cache-write (miss / creation) input tokens, in USD.
    /// Anthropic: input × 1.25. OpenAI/Gemini: equal to input (no surcharge).
    pub cache_write_cost_per_m: f64,
    /// Provider identifier (matches ProviderType variants).
    pub provider: &'static str,
}

/// All known models across all providers.
///
/// Last updated: June 2026 (OpenRouter section re-verified against the live
/// catalogue on 2026-06-10).
pub static KNOWN_MODELS: &[ModelSpec] = &[
    // Anthropic (Claude) — https://platform.claude.com/docs/en/about-claude/pricing
    // cache_read = 0.10 × input, cache_write = 1.25 × input
    // Verified against the claude-api reference 2026-06-10. Bare aliases only —
    // Anthropic 404s made-up dated forms.

    // Fable 5 (current top tier, above Opus)
    ModelSpec {
        id: "claude-fable-5",
        name: "Claude Fable 5",
        max_output_tokens: 128_000,
        context_window: 1_000_000,
        input_cost_per_m: 10.0,
        output_cost_per_m: 50.0,
        cache_read_cost_per_m: 1.0,
        cache_write_cost_per_m: 12.5,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-opus-4-7",
        name: "Claude Opus 4.7",
        max_output_tokens: 128_000,
        context_window: 1_000_000,
        input_cost_per_m: 5.0,
        output_cost_per_m: 25.0,
        cache_read_cost_per_m: 0.50,
        cache_write_cost_per_m: 6.25,
        provider: "Claude",
    },
    // 4.6 — use the bare aliases. Anthropic's API rejects the dated form
    // `claude-{model}-4-6-20260401` with a 404 not_found_error; the undated
    // alias is what's actually live. (Mirrors the 4.7 entry above, which
    // already uses the bare `claude-opus-4-7` alias and works.)
    ModelSpec {
        id: "claude-opus-4-6",
        name: "Claude Opus 4.6",
        max_output_tokens: 128_000,
        context_window: 1_000_000,
        input_cost_per_m: 5.0,
        output_cost_per_m: 25.0,
        cache_read_cost_per_m: 0.50,
        cache_write_cost_per_m: 6.25,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-opus-4-8",
        name: "Claude Opus 4.8",
        max_output_tokens: 128_000,
        context_window: 1_000_000,
        input_cost_per_m: 5.0,
        output_cost_per_m: 25.0,
        cache_read_cost_per_m: 0.50,
        cache_write_cost_per_m: 6.25,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-sonnet-4-6",
        name: "Claude Sonnet 4.6",
        max_output_tokens: 64_000,
        context_window: 1_000_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        cache_read_cost_per_m: 0.30,
        cache_write_cost_per_m: 3.75,
        provider: "Claude",
    },
    // 4.5 — the dated full id is claude-sonnet-4-5-20250929; the bare alias
    // matches both forms via prefix lookup.
    ModelSpec {
        id: "claude-sonnet-4-5",
        name: "Claude Sonnet 4.5",
        max_output_tokens: 64_000,
        context_window: 200_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        cache_read_cost_per_m: 0.30,
        cache_write_cost_per_m: 3.75,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-haiku-4-5-20251001",
        name: "Claude Haiku 4.5",
        max_output_tokens: 64_000,
        context_window: 200_000,
        input_cost_per_m: 1.0,
        output_cost_per_m: 5.0,
        cache_read_cost_per_m: 0.10,
        cache_write_cost_per_m: 1.25,
        provider: "Claude",
    },
    // (Claude 4.0 entries removed — both retire 2026-06-15.)

    // OpenAI — https://developers.openai.com/api/docs/pricing
    // GPT-5/Codex: cache_read = 0.10×, GPT-4o: 0.50×; no write surcharge

    // GPT-5.4 family (current flagship, March 2026)
    ModelSpec {
        id: "gpt-5.4-pro",
        name: "GPT-5.4 Pro",
        max_output_tokens: 128_000,
        context_window: 1_048_576,
        input_cost_per_m: 30.0,
        output_cost_per_m: 180.0,
        cache_read_cost_per_m: 3.0,
        cache_write_cost_per_m: 30.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5.4",
        name: "GPT-5.4",
        max_output_tokens: 128_000,
        context_window: 1_048_576,
        input_cost_per_m: 2.50,
        output_cost_per_m: 15.0,
        cache_read_cost_per_m: 0.25,
        cache_write_cost_per_m: 2.50,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5.4-mini",
        name: "GPT-5.4 Mini",
        max_output_tokens: 128_000,
        context_window: 400_000,
        input_cost_per_m: 0.75,
        output_cost_per_m: 4.50,
        cache_read_cost_per_m: 0.075,
        cache_write_cost_per_m: 0.75,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5.4-nano",
        name: "GPT-5.4 Nano",
        max_output_tokens: 128_000,
        context_window: 400_000,
        input_cost_per_m: 0.20,
        output_cost_per_m: 1.25,
        cache_read_cost_per_m: 0.02,
        cache_write_cost_per_m: 0.20,
        provider: "OpenAi",
    },

    // Codex models (agentic coding)
    ModelSpec {
        id: "gpt-5.3-codex",
        name: "GPT-5.3 Codex",
        max_output_tokens: 128_000,
        context_window: 400_000,
        input_cost_per_m: 1.75,
        output_cost_per_m: 14.0,
        cache_read_cost_per_m: 0.175,
        cache_write_cost_per_m: 1.75,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-5-codex",
        name: "GPT-5 Codex",
        max_output_tokens: 128_000,
        context_window: 400_000,
        input_cost_per_m: 1.25,
        output_cost_per_m: 10.0,
        cache_read_cost_per_m: 0.125,
        cache_write_cost_per_m: 1.25,
        provider: "OpenAi",
    },

    // Reasoning models
    ModelSpec {
        id: "o4-mini",
        name: "o4-mini",
        max_output_tokens: 100_000,
        context_window: 200_000,
        input_cost_per_m: 1.10,
        output_cost_per_m: 4.40,
        cache_read_cost_per_m: 0.275,
        cache_write_cost_per_m: 1.10,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "o3",
        name: "o3",
        max_output_tokens: 100_000,
        context_window: 200_000,
        input_cost_per_m: 2.0,
        output_cost_per_m: 8.0,
        cache_read_cost_per_m: 0.50,
        cache_write_cost_per_m: 2.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "o3-mini",
        name: "o3-mini",
        max_output_tokens: 100_000,
        context_window: 200_000,
        input_cost_per_m: 1.10,
        output_cost_per_m: 4.40,
        cache_read_cost_per_m: 0.275,
        cache_write_cost_per_m: 1.10,
        provider: "OpenAi",
    },

    // GPT-4.1 family (legacy)
    ModelSpec {
        id: "gpt-4.1",
        name: "GPT-4.1",
        max_output_tokens: 32_768,
        context_window: 1_048_576,
        input_cost_per_m: 2.0,
        output_cost_per_m: 8.0,
        cache_read_cost_per_m: 0.50,
        cache_write_cost_per_m: 2.0,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-4.1-mini",
        name: "GPT-4.1 Mini",
        max_output_tokens: 32_768,
        context_window: 1_048_576,
        input_cost_per_m: 0.40,
        output_cost_per_m: 1.60,
        cache_read_cost_per_m: 0.10,
        cache_write_cost_per_m: 0.40,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-4.1-nano",
        name: "GPT-4.1 Nano",
        max_output_tokens: 32_768,
        context_window: 1_048_576,
        input_cost_per_m: 0.10,
        output_cost_per_m: 0.40,
        cache_read_cost_per_m: 0.025,
        cache_write_cost_per_m: 0.10,
        provider: "OpenAi",
    },

    // GPT-4o family (legacy)
    ModelSpec {
        id: "gpt-4o",
        name: "GPT-4o",
        max_output_tokens: 16_384,
        context_window: 128_000,
        input_cost_per_m: 2.50,
        output_cost_per_m: 10.0,
        cache_read_cost_per_m: 1.25,
        cache_write_cost_per_m: 2.50,
        provider: "OpenAi",
    },
    ModelSpec {
        id: "gpt-4o-mini",
        name: "GPT-4o Mini",
        max_output_tokens: 16_384,
        context_window: 128_000,
        input_cost_per_m: 0.15,
        output_cost_per_m: 0.60,
        cache_read_cost_per_m: 0.075,
        cache_write_cost_per_m: 0.15,
        provider: "OpenAi",
    },

    // Google Gemini — https://ai.google.dev/gemini-api/docs/pricing
    // Curated set (user decision, 2026-06-10): Gemini 3.5 Flash, 3.1 Pro,
    // 3.1 Flash-Lite. Verified against the live pricing page 2026-06-10.
    // cache_read = the "context caching" price (~0.10 × input); cache_write
    // = input (Gemini bills cache storage separately per hour, which we
    // don't model). Pro tiers double above 200K-token prompts; we record
    // the ≤200K price.

    // 3.5 Flash — current flagship flash (agentic/coding tier)
    ModelSpec {
        id: "gemini-3.5-flash",
        name: "Gemini 3.5 Flash",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 1.50,
        output_cost_per_m: 9.0,
        cache_read_cost_per_m: 0.15,
        cache_write_cost_per_m: 1.50,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-3.1-pro",
        name: "Gemini 3.1 Pro",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 2.0,
        output_cost_per_m: 12.0,
        cache_read_cost_per_m: 0.20,
        cache_write_cost_per_m: 2.0,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-3.1-flash-lite",
        name: "Gemini 3.1 Flash-Lite",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 0.25,
        output_cost_per_m: 1.50,
        cache_read_cost_per_m: 0.025,
        cache_write_cost_per_m: 0.25,
        provider: "Gemini",
    },

    // OpenRouter — via https://openrouter.ai/api/v1
    //
    // Curated pre-configured set (user decision, 2026-06-10): DeepSeek V4 /
    // V4 Flash, Qwen3.7 Max / Plus, GLM 5, Kimi K2, Kimi K2.6, MiMo V2.5,
    // MiMo V2.5 Pro, MiniMax M2.5, MiniMax M3. Anything else the user
    // registers themselves (and OpenRouter models are auto-enriched from the
    // live catalogue at pick time anyway, so removed entries still get
    // accurate cost/context via `fetch_openrouter_model_specs`).
    //
    // Pricing verified against the live OpenRouter catalogue on 2026-06-10
    // (GET /api/v1/models; prompt/completion/input_cache_read per-token × 1M).
    // cache_write stays 0.0 — these providers report no write surcharge.
    // max_output_tokens is clamped below the catalogue value where the
    // reported max equals (or nearly equals) the context window, since
    // reserving the full window for output would starve input + condense math.

    // DeepSeek V4 — 1M-token context on both tiers
    ModelSpec {
        id: "deepseek/deepseek-v4-pro",
        name: "DeepSeek V4 Pro",
        max_output_tokens: 131_072,
        context_window: 1_048_576,
        input_cost_per_m: 0.435,
        output_cost_per_m: 0.87,
        cache_read_cost_per_m: 0.0036,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    ModelSpec {
        id: "deepseek/deepseek-v4-flash",
        name: "DeepSeek V4 Flash",
        max_output_tokens: 131_072,
        context_window: 1_048_576,
        input_cost_per_m: 0.0983,
        output_cost_per_m: 0.1966,
        cache_read_cost_per_m: 0.0197,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // Alibaba Qwen 3.7 — current flagship generation, 1M-token context
    ModelSpec {
        id: "qwen/qwen3.7-max",
        name: "Qwen3.7 Max",
        max_output_tokens: 65_536,
        context_window: 1_000_000,
        input_cost_per_m: 1.25,
        output_cost_per_m: 3.75,
        cache_read_cost_per_m: 0.25,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    ModelSpec {
        id: "qwen/qwen3.7-plus",
        name: "Qwen3.7 Plus",
        max_output_tokens: 65_536,
        context_window: 1_000_000,
        input_cost_per_m: 0.40,
        output_cost_per_m: 1.60,
        cache_read_cost_per_m: 0.08,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // Z.AI GLM 5 — ctx 202_752 per catalogue; max output unreported, sibling
    // GLM models report 131_072 — clamped to 64K for a sane input/output split.
    ModelSpec {
        id: "z-ai/glm-5",
        name: "GLM 5",
        max_output_tokens: 65_536,
        context_window: 202_752,
        input_cost_per_m: 0.60,
        output_cost_per_m: 1.92,
        cache_read_cost_per_m: 0.12,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // Moonshot Kimi K2 (0711 base release)
    ModelSpec {
        id: "moonshotai/kimi-k2",
        name: "Kimi K2",
        max_output_tokens: 32_768,
        context_window: 131_072,
        input_cost_per_m: 0.57,
        output_cost_per_m: 2.30,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // Kimi K2.6 — catalogue reports max output = full 262K context; clamped.
    ModelSpec {
        id: "moonshotai/kimi-k2.6",
        name: "Kimi K2.6",
        max_output_tokens: 65_536,
        context_window: 262_144,
        input_cost_per_m: 0.68,
        output_cost_per_m: 3.41,
        cache_read_cost_per_m: 0.34,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // Xiaomi MiMo V2.5 — 1M-token context
    ModelSpec {
        id: "xiaomi/mimo-v2.5",
        name: "MiMo V2.5",
        max_output_tokens: 131_072,
        context_window: 1_048_576,
        input_cost_per_m: 0.14,
        output_cost_per_m: 0.28,
        cache_read_cost_per_m: 0.0028,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    ModelSpec {
        id: "xiaomi/mimo-v2.5-pro",
        name: "MiMo V2.5 Pro",
        max_output_tokens: 131_072,
        context_window: 1_048_576,
        input_cost_per_m: 0.435,
        output_cost_per_m: 0.87,
        cache_read_cost_per_m: 0.0036,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // MiniMax M2.5 — catalogue reports max output 196_608 of a 204_800
    // context; clamped.
    ModelSpec {
        id: "minimax/minimax-m2.5",
        name: "MiniMax M2.5",
        max_output_tokens: 65_536,
        context_window: 204_800,
        input_cost_per_m: 0.15,
        output_cost_per_m: 0.90,
        cache_read_cost_per_m: 0.05,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // MiniMax M3 — 1M-token context; catalogue reports max output 512_000,
    // clamped to 128K.
    ModelSpec {
        id: "minimax/minimax-m3",
        name: "MiniMax M3",
        max_output_tokens: 131_072,
        context_window: 1_048_576,
        input_cost_per_m: 0.30,
        output_cost_per_m: 1.20,
        cache_read_cost_per_m: 0.06,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },


    // OpenRouter meta-models — dynamic routing / free tier
    // These don't have fixed specs, but we define safe defaults to prevent
    // the API from returning nonsensical values (e.g. max_output=2M).
    ModelSpec {
        id: "openrouter/auto",
        name: "OpenRouter Auto",
        max_output_tokens: 16_384,
        context_window: 200_000,
        input_cost_per_m: 0.0,
        output_cost_per_m: 0.0,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    ModelSpec {
        id: "openrouter/free",
        name: "OpenRouter Free",
        max_output_tokens: 16_384,
        context_window: 128_000,
        input_cost_per_m: 0.0,
        output_cost_per_m: 0.0,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
];

/// Look up a model by its ID. Tries exact match first, then bidirectional prefix match:
/// - Forward: model_id starts with m.id  (user gave a more specific version, e.g. with date suffix)
/// - Reverse: m.id starts with model_id  (user gave a shorter alias, e.g. "claude-opus-4-6"
///                                         matches registry entry "claude-opus-4-6-20260401")
/// Longest common prefix wins to pick the most specific match.
pub fn lookup(model_id: &str) -> Option<&'static ModelSpec> {
    if let Some(spec) = KNOWN_MODELS.iter().find(|m| m.id == model_id) {
        return Some(spec);
    }
    KNOWN_MODELS
        .iter()
        .filter(|m| model_id.starts_with(m.id) || m.id.starts_with(model_id))
        .max_by_key(|m| {
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

/// Get the context window for a model. Returns the registry value if known,
/// or `fallback` for unknown models.
pub fn context_window(model_id: &str, fallback: u32) -> u32 {
    lookup(model_id).map(|m| m.context_window).unwrap_or(fallback)
}

/// Get all known models for a given provider (e.g. "Claude", "OpenAi", "Gemini").
pub fn models_for_provider(provider: &str) -> Vec<&'static ModelSpec> {
    KNOWN_MODELS
        .iter()
        .filter(|m| m.provider == provider)
        .collect()
}
