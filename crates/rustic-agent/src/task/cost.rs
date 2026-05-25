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

    // Per-category USD breakdown. `estimated_cost_usd` is the sum of these
    // (plus subagent_cost_usd). Older serialized snapshots without these
    // fields default to 0 — the rolled-up total still reads correctly.
    #[serde(default)]
    pub input_cost_usd: f64,
    #[serde(default)]
    pub output_cost_usd: f64,
    #[serde(default)]
    pub cache_read_cost_usd: f64,
    #[serde(default)]
    pub cache_write_cost_usd: f64,
    #[serde(default)]
    pub image_cost_usd: f64,
    #[serde(default)]
    pub video_cost_usd: f64,
    #[serde(default)]
    pub subagent_cost_usd: f64,
}

/// Per-call USD breakdown across the four LLM token categories. Used by
/// `add_turn` to split the cost it bills into individual category buckets so
/// the UI can show "of the X dollars, this much was cache read".
#[derive(Debug, Clone, Default, Copy)]
pub struct CostBreakdown {
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
}

impl CostBreakdown {
    pub fn total(&self) -> f64 {
        self.input_usd + self.output_usd + self.cache_read_usd + self.cache_write_usd
    }
}

impl TaskCost {
    pub fn add_turn(&mut self, model: &str, usage: &TokenUsage) {
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;
        self.total_cache_read_tokens += usage.cache_read_tokens as u64;
        self.total_cache_write_tokens += usage.cache_write_tokens as u64;
        let breakdown = calculate_cost_breakdown(model, usage);
        self.input_cost_usd += breakdown.input_usd;
        self.output_cost_usd += breakdown.output_usd;
        self.cache_read_cost_usd += breakdown.cache_read_usd;
        self.cache_write_cost_usd += breakdown.cache_write_usd;
        self.estimated_cost_usd += breakdown.total();
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
        self.input_cost_usd += other.input_cost_usd;
        self.output_cost_usd += other.output_cost_usd;
        self.cache_read_cost_usd += other.cache_read_cost_usd;
        self.cache_write_cost_usd += other.cache_write_cost_usd;
        self.image_cost_usd += other.image_cost_usd;
        self.video_cost_usd += other.video_cost_usd;
        self.subagent_cost_usd += other.subagent_cost_usd;
        self.turn_count = self.turn_count.saturating_add(other.turn_count);
    }
}

/// Returns estimated USD cost for a single provider call given the model and token usage.
/// Convenience wrapper around `calculate_cost_breakdown` for callers that only need the total.
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> f64 {
    calculate_cost_breakdown(model, usage).total()
}

/// Per-category breakdown of the USD cost for a single provider call. All four
/// token categories (input, output, cache_read, cache_write) are billed at
/// independent per-model rates pulled from `model_registry`.
pub fn calculate_cost_breakdown(model: &str, usage: &TokenUsage) -> CostBreakdown {
    let (input_per_m, output_per_m, cache_read_per_m, cache_write_per_m) = pricing(model);
    CostBreakdown {
        input_usd: (usage.input_tokens as f64 / 1_000_000.0) * input_per_m,
        output_usd: (usage.output_tokens as f64 / 1_000_000.0) * output_per_m,
        cache_read_usd: (usage.cache_read_tokens as f64 / 1_000_000.0) * cache_read_per_m,
        cache_write_usd: (usage.cache_write_tokens as f64 / 1_000_000.0) * cache_write_per_m,
    }
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
