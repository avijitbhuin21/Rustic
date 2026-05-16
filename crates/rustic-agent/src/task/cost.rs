use crate::model_registry;
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

    /// P1.8: accumulate `other` into self. Used by the goal-loop wrapper
    /// to fold per-iteration `run_turn` cost into a single aggregate
    /// surface for the host.
    pub fn merge_into(&mut self, other: &TaskCost) {
        self.total_input_tokens += other.total_input_tokens;
        self.total_output_tokens += other.total_output_tokens;
        self.total_cache_read_tokens += other.total_cache_read_tokens;
        self.total_cache_write_tokens += other.total_cache_write_tokens;
        self.estimated_cost_usd += other.estimated_cost_usd;
        self.turn_count = self.turn_count.saturating_add(other.turn_count);
    }
}

/// Returns estimated USD cost for a single provider call given the model and token usage.
/// All four token categories (input, output, cache_read, cache_write) are billed separately
/// at the per-model rates defined in `model_registry`.
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> f64 {
    let (input_per_m, output_per_m, cache_read_per_m, cache_write_per_m) = pricing(model);

    let input_cost = (usage.input_tokens as f64 / 1_000_000.0) * input_per_m;
    let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * output_per_m;
    let cache_read_cost = (usage.cache_read_tokens as f64 / 1_000_000.0) * cache_read_per_m;
    let cache_write_cost = (usage.cache_write_tokens as f64 / 1_000_000.0) * cache_write_per_m;

    input_cost + output_cost + cache_read_cost + cache_write_cost
}

/// Returns (input, output, cache_read, cache_write) $/MTok for the given model id.
/// Sources prices from `model_registry::lookup`. Unknown models fall back to
/// Sonnet-4-class pricing so the estimate is in the right ballpark rather than zero.
fn pricing(model: &str) -> (f64, f64, f64, f64) {
    if let Some(spec) = model_registry::lookup(model) {
        return (
            spec.input_cost_per_m,
            spec.output_cost_per_m,
            spec.cache_read_cost_per_m,
            spec.cache_write_cost_per_m,
        );
    }
    // Fallback: mid-tier Sonnet-class pricing with Anthropic cache conventions.
    (3.0, 15.0, 0.30, 3.75)
}
