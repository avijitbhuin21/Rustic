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

    /// Sum of provider-reported authoritative costs (OpenRouter `usage.cost`),
    /// when available. For turns that reported it, `estimated_cost_usd` already
    /// equals this exact figure; this field records how much of the running
    /// total was billed from the provider's number rather than tokens × price.
    #[serde(default)]
    pub actual_cost_usd: Option<f64>,
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
    pub fn add_turn(
        &mut self,
        model: &str,
        usage: &TokenUsage,
        custom_input_cost: Option<f64>,
        custom_output_cost: Option<f64>,
        custom_cache_read_cost: Option<f64>,
        custom_cache_write_cost: Option<f64>,
        // Provider-reported authoritative cost for this turn (OpenRouter
        // `usage.cost`). When `Some`, it's billed as the turn total instead of
        // the token × price estimate.
        actual_cost: Option<f64>,
    ) {
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;
        self.total_cache_read_tokens += usage.cache_read_tokens as u64;
        self.total_cache_write_tokens += usage.cache_write_tokens as u64;
        let breakdown = calculate_cost_breakdown(
            model,
            usage,
            custom_input_cost,
            custom_output_cost,
            custom_cache_read_cost,
            custom_cache_write_cost,
        );
        // The per-category buckets stay token-based (used only for the "of the
        // total, this much was cache read" display). The headline total prefers
        // the provider's authoritative figure when present — OpenRouter routes
        // one model id to different-priced upstreams, so tokens × price can't
        // reconstruct it. This makes mid-conversation provider switches bill
        // correctly with no extra bookkeeping.
        self.input_cost_usd += breakdown.input_usd;
        self.output_cost_usd += breakdown.output_usd;
        self.cache_read_cost_usd += breakdown.cache_read_usd;
        self.cache_write_cost_usd += breakdown.cache_write_usd;
        match actual_cost {
            Some(c) => {
                self.estimated_cost_usd += c;
                self.actual_cost_usd = Some(self.actual_cost_usd.unwrap_or(0.0) + c);
            }
            None => {
                self.estimated_cost_usd += breakdown.total();
            }
        }
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
        if self.actual_cost_usd.is_some() || other.actual_cost_usd.is_some() {
            self.actual_cost_usd =
                Some(self.actual_cost_usd.unwrap_or(0.0) + other.actual_cost_usd.unwrap_or(0.0));
        }
        self.turn_count = self.turn_count.saturating_add(other.turn_count);
    }
}

/// Returns estimated USD cost for a single provider call given the model and token usage.
/// Convenience wrapper around `calculate_cost_breakdown` for callers that only need the total.
pub fn calculate_cost(
    model: &str,
    usage: &TokenUsage,
    custom_input_cost: Option<f64>,
    custom_output_cost: Option<f64>,
    custom_cache_read_cost: Option<f64>,
    custom_cache_write_cost: Option<f64>,
) -> f64 {
    calculate_cost_breakdown(model, usage, custom_input_cost, custom_output_cost, custom_cache_read_cost, custom_cache_write_cost).total()
}

/// Per-category breakdown of the USD cost for a single provider call. All four
/// token categories (input, output, cache_read, cache_write) are billed at
/// independent per-model rates pulled from custom config (if provided) or `model_registry`.
pub fn calculate_cost_breakdown(
    model: &str,
    usage: &TokenUsage,
    custom_input_cost: Option<f64>,
    custom_output_cost: Option<f64>,
    custom_cache_read_cost: Option<f64>,
    custom_cache_write_cost: Option<f64>,
) -> CostBreakdown {
    let (input_per_m, output_per_m, cache_read_per_m, cache_write_per_m) = 
        pricing(model, custom_input_cost, custom_output_cost, custom_cache_read_cost, custom_cache_write_cost);
    CostBreakdown {
        input_usd: (usage.input_tokens as f64 / 1_000_000.0) * input_per_m,
        output_usd: (usage.output_tokens as f64 / 1_000_000.0) * output_per_m,
        cache_read_usd: (usage.cache_read_tokens as f64 / 1_000_000.0) * cache_read_per_m,
        cache_write_usd: (usage.cache_write_tokens as f64 / 1_000_000.0) * cache_write_per_m,
    }
}

/// Returns (input, output, cache_read, cache_write) $/MTok for the given model id.
/// Checks custom pricing first (when provided), then sources from `model_registry::lookup`.
/// Unknown models fall back to Sonnet-4-class pricing so the estimate is in the right
/// ballpark rather than zero.
fn pricing(
    model: &str,
    custom_input: Option<f64>,
    custom_output: Option<f64>,
    custom_cache_read: Option<f64>,
    custom_cache_write: Option<f64>,
) -> (f64, f64, f64, f64) {
    // Priority 1: Use custom pricing if all four values are provided
    if let (Some(inp), Some(out), Some(cr), Some(cw)) = 
        (custom_input, custom_output, custom_cache_read, custom_cache_write) {
        return (inp, out, cr, cw);
    }
    
    // Priority 2: Check static model registry
    if let Some(spec) = model_registry::lookup(model) {
        return (
            spec.input_cost_per_m,
            spec.output_cost_per_m,
            spec.cache_read_cost_per_m,
            spec.cache_write_cost_per_m,
        );
    }
    
    // Priority 3: Fallback to mid-tier Sonnet-class pricing with Anthropic cache conventions.
    (3.0, 15.0, 0.30, 3.75)
}
