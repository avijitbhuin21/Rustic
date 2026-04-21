use crate::provider::TokenUsage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskCost {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    #[serde(default)]
    pub total_cache_write_tokens: u64,
    pub estimated_cost_usd: f64,
    pub turn_count: u32,
}

impl TaskCost {
    pub fn add_turn(&mut self, model: &str, usage: &TokenUsage) {
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;
        self.total_cache_read_tokens += usage.cache_read_tokens as u64;
        self.total_cache_write_tokens += usage.cache_write_tokens as u64;
        self.estimated_cost_usd += calculate_cost(model, usage);
        self.turn_count += 1;
    }
}

/// Returns estimated USD cost for a single provider call given the model and token usage.
/// Prices are per-million tokens (MTok).
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> f64 {
    let (input_per_mtok, output_per_mtok, cache_read_per_mtok) = pricing(model);
    // Cache write is 1.25× the normal input rate on Anthropic; 0 elsewhere.
    let cache_write_per_mtok = if model.contains("claude") {
        input_per_mtok * 1.25
    } else {
        input_per_mtok
    };

    let input_cost = (usage.input_tokens as f64 / 1_000_000.0) * input_per_mtok;
    let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * output_per_mtok;
    let cache_read_cost = (usage.cache_read_tokens as f64 / 1_000_000.0) * cache_read_per_mtok;
    let cache_write_cost = (usage.cache_write_tokens as f64 / 1_000_000.0) * cache_write_per_mtok;

    input_cost + output_cost + cache_read_cost + cache_write_cost
}

/// Returns (input_$/MTok, output_$/MTok, cache_read_$/MTok) for the given model id.
/// Unknown models fall back to claude-sonnet-4 pricing.
fn pricing(model: &str) -> (f64, f64, f64) {
    // Claude models
    if model.contains("claude-opus-4") {
        return (15.0, 75.0, 1.50);
    }
    if model.contains("claude-sonnet-4") || model.contains("claude-3-7-sonnet") {
        return (3.0, 15.0, 0.30);
    }
    if model.contains("claude-3-5-sonnet") {
        return (3.0, 15.0, 0.30);
    }
    if model.contains("claude-3-5-haiku") || model.contains("claude-haiku-4") {
        return (0.80, 4.0, 0.08);
    }
    if model.contains("claude-3-opus") {
        return (15.0, 75.0, 1.50);
    }
    if model.contains("claude-3-haiku") {
        return (0.25, 1.25, 0.03);
    }

    // OpenAI models
    if model.starts_with("gpt-4o") && model.contains("mini") {
        return (0.15, 0.60, 0.075);
    }
    if model.starts_with("gpt-4o") {
        return (2.50, 10.0, 1.25);
    }
    if model.starts_with("gpt-4-turbo") || model.starts_with("gpt-4-1106") {
        return (10.0, 30.0, 0.0);
    }
    if model.starts_with("o1-mini") {
        return (1.10, 4.40, 0.55);
    }
    if model.starts_with("o1") {
        return (15.0, 60.0, 7.50);
    }
    if model.starts_with("o3-mini") {
        return (1.10, 4.40, 0.55);
    }
    if model.starts_with("o3") {
        return (10.0, 40.0, 2.50);
    }

    // Gemini models
    if model.contains("gemini-2.0-flash") {
        return (0.10, 0.40, 0.025);
    }
    if model.contains("gemini-1.5-flash") {
        return (0.075, 0.30, 0.01875);
    }
    if model.contains("gemini-1.5-pro") {
        return (1.25, 5.0, 0.3125);
    }

    // Default: Sonnet 4 pricing
    (3.0, 15.0, 0.30)
}
