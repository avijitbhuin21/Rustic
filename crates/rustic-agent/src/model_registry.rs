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
/// Last updated: April 2026.
pub static KNOWN_MODELS: &[ModelSpec] = &[
    // ═══════════════════════════════════════════════════════════════════════════
    // Anthropic (Claude) — https://platform.claude.com/docs/en/about-claude/pricing
    // ═══════════════════════════════════════════════════════════════════════════

    // Claude cache pricing convention (Anthropic API):
    //   cache_read  = 0.10 × input   (90% discount on cache hits)
    //   cache_write = 1.25 × input   (5-min TTL ephemeral breakpoint)

    // ── 4.7 (current flagship, April 2026) ───────────────────────────────────
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
    // ── 4.6 ──────────────────────────────────────────────────────────────────
    ModelSpec {
        id: "claude-opus-4-6-20260401",
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
        id: "claude-sonnet-4-6-20260401",
        name: "Claude Sonnet 4.6",
        max_output_tokens: 64_000,
        context_window: 1_000_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        cache_read_cost_per_m: 0.30,
        cache_write_cost_per_m: 3.75,
        provider: "Claude",
    },
    // ── 4.5 ──────────────────────────────────────────────────────────────────
    ModelSpec {
        id: "claude-sonnet-4-5-20250514",
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
    // ── 4.0 (legacy, still in API) ───────────────────────────────────────────
    ModelSpec {
        id: "claude-opus-4-20250514",
        name: "Claude Opus 4",
        max_output_tokens: 128_000,
        context_window: 200_000,
        input_cost_per_m: 5.0,
        output_cost_per_m: 25.0,
        cache_read_cost_per_m: 0.50,
        cache_write_cost_per_m: 6.25,
        provider: "Claude",
    },
    ModelSpec {
        id: "claude-sonnet-4-20250514",
        name: "Claude Sonnet 4",
        max_output_tokens: 64_000,
        context_window: 200_000,
        input_cost_per_m: 3.0,
        output_cost_per_m: 15.0,
        cache_read_cost_per_m: 0.30,
        cache_write_cost_per_m: 3.75,
        provider: "Claude",
    },

    // ═══════════════════════════════════════════════════════════════════════════
    // OpenAI — https://developers.openai.com/api/docs/pricing
    // ═══════════════════════════════════════════════════════════════════════════

    // OpenAI cache pricing convention:
    //   GPT-5 / Codex / reasoning: cache_read = 0.10 × input (90% discount)
    //   GPT-4o family:             cache_read = 0.50 × input (50% discount)
    //   cache_write                = input (no write surcharge)

    // ── GPT-5.4 family (current flagship, March 2026) ────────────────────────
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

    // ── Codex models (agentic coding) ────────────────────────────────────────
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

    // ── Reasoning models ─────────────────────────────────────────────────────
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

    // ── GPT-4.1 family (legacy) ──────────────────────────────────────────────
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

    // ── GPT-4o family (legacy) ───────────────────────────────────────────────
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

    // ═══════════════════════════════════════════════════════════════════════════
    // Google Gemini — https://ai.google.dev/gemini-api/docs/pricing
    // ═══════════════════════════════════════════════════════════════════════════

    // Gemini cache pricing convention:
    //   cache_read  = 0.25 × input   (75% discount on cached tokens)
    //   cache_write = input

    // ── 3.1 family (current flagship, Feb 2026) ─────────────────────────────
    ModelSpec {
        id: "gemini-3.1-pro",
        name: "Gemini 3.1 Pro",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 2.0,
        output_cost_per_m: 12.0,
        cache_read_cost_per_m: 0.50,
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
        cache_read_cost_per_m: 0.0625,
        cache_write_cost_per_m: 0.25,
        provider: "Gemini",
    },

    // ── 3.0 family ───────────────────────────────────────────────────────────
    ModelSpec {
        id: "gemini-3-flash",
        name: "Gemini 3 Flash",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 0.50,
        output_cost_per_m: 3.0,
        cache_read_cost_per_m: 0.125,
        cache_write_cost_per_m: 0.50,
        provider: "Gemini",
    },

    // ── 2.5 family (still widely used) ───────────────────────────────────────
    ModelSpec {
        id: "gemini-2.5-pro",
        name: "Gemini 2.5 Pro",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 1.25,
        output_cost_per_m: 10.0,
        cache_read_cost_per_m: 0.3125,
        cache_write_cost_per_m: 1.25,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-2.5-flash",
        name: "Gemini 2.5 Flash",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 0.15,
        output_cost_per_m: 0.60,
        cache_read_cost_per_m: 0.0375,
        cache_write_cost_per_m: 0.15,
        provider: "Gemini",
    },
    ModelSpec {
        id: "gemini-2.5-flash-lite",
        name: "Gemini 2.5 Flash-Lite",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 0.10,
        output_cost_per_m: 0.40,
        cache_read_cost_per_m: 0.025,
        cache_write_cost_per_m: 0.10,
        provider: "Gemini",
    },

    // ── 2.0 (deprecated June 2026, still in API) ────────────────────────────
    ModelSpec {
        id: "gemini-2.0-flash",
        name: "Gemini 2.0 Flash",
        max_output_tokens: 8_192,
        context_window: 1_048_576,
        input_cost_per_m: 0.10,
        output_cost_per_m: 0.40,
        cache_read_cost_per_m: 0.025,
        cache_write_cost_per_m: 0.10,
        provider: "Gemini",
    },

    // ── OpenRouter — via https://openrouter.ai/api/v1 ──────────────────────────
    //
    // OpenRouter doesn't bill cache hits separately (their billing layer is
    // pass-through), so cache_read / cache_write stay at 0.0 — the cost pill
    // on every cache event will show the regular per-token price.
    //
    // Pricing & context window numbers below were validated against
    // https://openrouter.ai/<vendor>/<slug> in May 2026. Add a new entry
    // here when OpenRouter ships a new Chinese-vendor model rather than
    // hand-editing user configs — this registry is what the frontend pulls
    // through `list_known_models` for the register-model template dropdown.

    // ─── DeepSeek (deepseek/*) ──────────────────────────────────────────────
    // R1 — open reasoning model. `reasoning` parameter exposes thinking tokens.
    ModelSpec {
        id: "deepseek/deepseek-r1",
        name: "DeepSeek R1",
        max_output_tokens: 32_768,
        context_window: 64_000,
        input_cost_per_m: 0.70,
        output_cost_per_m: 2.50,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // V3.1 — instruct model, no reasoning.
    ModelSpec {
        id: "deepseek/deepseek-chat-v3.1",
        name: "DeepSeek V3.1",
        max_output_tokens: 16_384,
        context_window: 32_768,
        input_cost_per_m: 0.15,
        output_cost_per_m: 0.75,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // V3.2 — current GA flagship, instruct. Slug `deepseek/deepseek-v3.2`.
    ModelSpec {
        id: "deepseek/deepseek-v3.2",
        name: "DeepSeek V3.2",
        max_output_tokens: 32_768,
        context_window: 131_072,
        input_cost_per_m: 0.252,
        output_cost_per_m: 0.378,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // V3.2-Exp — experimental cousin of V3.2 published earlier.
    ModelSpec {
        id: "deepseek/deepseek-v3.2-exp",
        name: "DeepSeek V3.2 Exp",
        max_output_tokens: 32_768,
        context_window: 131_072,
        input_cost_per_m: 0.252,
        output_cost_per_m: 0.378,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // V3 chat (older alias still supported). Kept so existing tasks that
    // selected `deepseek/deepseek-chat` resolve.
    ModelSpec {
        id: "deepseek/deepseek-chat",
        name: "DeepSeek V3 Chat",
        max_output_tokens: 32_768,
        context_window: 131_072,
        input_cost_per_m: 0.27,
        output_cost_per_m: 1.10,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // ─── Moonshot Kimi (moonshotai/*) ───────────────────────────────────────
    // K2 — original 0711 release. Instruct.
    ModelSpec {
        id: "moonshotai/kimi-k2",
        name: "Kimi K2 (0711)",
        max_output_tokens: 16_384,
        context_window: 131_072,
        input_cost_per_m: 1.00,
        output_cost_per_m: 3.00,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // K2 0905 — refreshed K2 with extended context.
    ModelSpec {
        id: "moonshotai/kimi-k2-0905",
        name: "Kimi K2 (0905)",
        max_output_tokens: 16_384,
        context_window: 262_144,
        input_cost_per_m: 1.00,
        output_cost_per_m: 3.00,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // K2 Thinking — agentic reasoning variant. Trillion-param MoE, 32B active.
    ModelSpec {
        id: "moonshotai/kimi-k2-thinking",
        name: "Kimi K2 Thinking",
        max_output_tokens: 32_768,
        context_window: 262_144,
        input_cost_per_m: 0.60,
        output_cost_per_m: 2.50,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // K2.6 — current flagship multimodal/coding model from Moonshot.
    ModelSpec {
        id: "moonshotai/kimi-k2.6",
        name: "Kimi K2.6",
        max_output_tokens: 16_384,
        context_window: 262_144,
        input_cost_per_m: 0.75,
        output_cost_per_m: 3.50,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // ─── Z.AI / Zhipu GLM (z-ai/*) ──────────────────────────────────────────
    // Note: the legacy `thudm/glm-4-32b` slug has been retired by OpenRouter
    // in favour of the `z-ai/*` namespace; older saved tasks pointing at it
    // will fall through prefix lookup to the closest GLM entry below.
    ModelSpec {
        id: "z-ai/glm-4.5",
        name: "GLM 4.5",
        max_output_tokens: 98_304,
        context_window: 131_072,
        input_cost_per_m: 0.60,
        output_cost_per_m: 2.20,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // GLM 4.5 Air — smaller, cheaper variant.
    ModelSpec {
        id: "z-ai/glm-4.5-air",
        name: "GLM 4.5 Air",
        max_output_tokens: 98_304,
        context_window: 131_072,
        input_cost_per_m: 0.20,
        output_cost_per_m: 1.10,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // GLM 4.6 — extended-context update of 4.5 with hybrid thinking.
    ModelSpec {
        id: "z-ai/glm-4.6",
        name: "GLM 4.6",
        max_output_tokens: 65_536,
        context_window: 204_800,
        input_cost_per_m: 0.39,
        output_cost_per_m: 1.90,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // GLM 4.7 — newest in the 4.x line with reasoning. Cheaper than 4.6.
    ModelSpec {
        id: "z-ai/glm-4.7",
        name: "GLM 4.7",
        max_output_tokens: 65_536,
        context_window: 202_752,
        input_cost_per_m: 0.40,
        output_cost_per_m: 1.75,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // GLM 5 — first model in the new 5.x family. Same hybrid-thinking shape
    // as the 4.x line but a clean version reset.
    ModelSpec {
        id: "z-ai/glm-5",
        name: "GLM 5",
        max_output_tokens: 65_536,
        context_window: 202_752,
        input_cost_per_m: 0.60,
        output_cost_per_m: 1.92,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // GLM 5.1 — current Z.AI flagship (released April 2026). Top-tier of
    // the GLM line, priced at $1.05 / $3.50.
    ModelSpec {
        id: "z-ai/glm-5.1",
        name: "GLM 5.1",
        max_output_tokens: 65_536,
        context_window: 202_752,
        input_cost_per_m: 1.05,
        output_cost_per_m: 3.50,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // ─── MiniMax (minimax/*) ────────────────────────────────────────────────
    // M1 — long-context (1M-token) model.
    ModelSpec {
        id: "minimax/minimax-m1",
        name: "MiniMax M1",
        max_output_tokens: 32_768,
        context_window: 1_000_000,
        input_cost_per_m: 0.40,
        output_cost_per_m: 2.20,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // M2 — current GA model.
    ModelSpec {
        id: "minimax/minimax-m2",
        name: "MiniMax M2",
        max_output_tokens: 65_536,
        context_window: 196_608,
        input_cost_per_m: 0.255,
        output_cost_per_m: 1.00,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // M2.5 — cheapest of the M2 line.
    ModelSpec {
        id: "minimax/minimax-m2.5",
        name: "MiniMax M2.5",
        max_output_tokens: 65_536,
        context_window: 196_608,
        input_cost_per_m: 0.15,
        output_cost_per_m: 1.15,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // M2.7 — newest tier, mid-priced.
    ModelSpec {
        id: "minimax/minimax-m2.7",
        name: "MiniMax M2.7",
        max_output_tokens: 65_536,
        context_window: 196_608,
        input_cost_per_m: 0.30,
        output_cost_per_m: 1.20,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // ─── Xiaomi MiMo (xiaomi/*) ─────────────────────────────────────────────
    // MiMo V2 Flash — budget tier.
    ModelSpec {
        id: "xiaomi/mimo-v2-flash",
        name: "MiMo V2 Flash",
        max_output_tokens: 16_384,
        context_window: 131_072,
        input_cost_per_m: 0.09,
        output_cost_per_m: 0.29,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // MiMo V2 Pro — flagship tier of the V2 family.
    ModelSpec {
        id: "xiaomi/mimo-v2-pro",
        name: "MiMo V2 Pro",
        max_output_tokens: 32_768,
        context_window: 131_072,
        input_cost_per_m: 1.00,
        output_cost_per_m: 3.00,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // MiMo V2.5 — Pro-level performance at half price.
    ModelSpec {
        id: "xiaomi/mimo-v2.5",
        name: "MiMo V2.5",
        max_output_tokens: 32_768,
        context_window: 131_072,
        input_cost_per_m: 0.40,
        output_cost_per_m: 2.00,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // MiMo V2.5 Pro — Xiaomi's flagship with 1M-token context.
    ModelSpec {
        id: "xiaomi/mimo-v2.5-pro",
        name: "MiMo V2.5 Pro",
        max_output_tokens: 65_536,
        context_window: 1_048_576,
        input_cost_per_m: 1.00,
        output_cost_per_m: 3.00,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },

    // ─── Qwen (qwen/*) ──────────────────────────────────────────────────────
    // Qwen 3.6 family — current Alibaba flagship line. All three support
    // reasoning via OpenRouter's `reasoning` parameter.
    // Max — top-tier preview, premium pricing for the highest reasoning lift.
    ModelSpec {
        id: "qwen/qwen3.6-max-preview",
        name: "Qwen3.6 Max",
        max_output_tokens: 65_536,
        context_window: 262_144,
        input_cost_per_m: 1.04,
        output_cost_per_m: 6.24,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // Plus — balanced general-purpose model with 1M context.
    ModelSpec {
        id: "qwen/qwen3.6-plus",
        name: "Qwen3.6 Plus",
        max_output_tokens: 65_536,
        context_window: 1_000_000,
        input_cost_per_m: 0.325,
        output_cost_per_m: 1.95,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // Flash — cheap/fast tier of the 3.6 family. Same 1M context as Plus.
    ModelSpec {
        id: "qwen/qwen3.6-flash",
        name: "Qwen3.6 Flash",
        max_output_tokens: 65_536,
        context_window: 1_000_000,
        input_cost_per_m: 0.25,
        output_cost_per_m: 1.50,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // Qwen3 235B A22B — MoE, 22B active. Strong general-purpose.
    ModelSpec {
        id: "qwen/qwen3-235b-a22b",
        name: "Qwen3 235B A22B",
        max_output_tokens: 16_384,
        context_window: 131_072,
        input_cost_per_m: 0.455,
        output_cost_per_m: 1.82,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // Qwen3 Coder 480B A35B — agentic-coding specialist with long context.
    ModelSpec {
        id: "qwen/qwen3-coder",
        name: "Qwen3 Coder 480B",
        max_output_tokens: 65_536,
        context_window: 262_144,
        input_cost_per_m: 0.22,
        output_cost_per_m: 1.80,
        cache_read_cost_per_m: 0.0,
        cache_write_cost_per_m: 0.0,
        provider: "OpenRouter",
    },
    // Qwen 2.5 72B Instruct — older but cheap general-purpose option.
    ModelSpec {
        id: "qwen/qwen-2.5-72b-instruct",
        name: "Qwen 2.5 72B",
        max_output_tokens: 16_384,
        context_window: 131_072,
        input_cost_per_m: 0.40,
        output_cost_per_m: 0.40,
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
