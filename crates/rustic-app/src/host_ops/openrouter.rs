//! OpenRouter catalogue specs and per-model provider stats, shared by both hosts.
//! Pricing arrives as USD-per-token strings and is converted to USD-per-1M; the
//! speed/latency enrichment uses an undocumented endpoint, so it is best-effort.

use super::models::MODEL_CACHE_TTL;

/// Parse one `pricing.<key>` field (USD per token, as a string) into USD per 1M.
///
/// OpenRouter uses `"-1"` as a sentinel for "price varies by routed model"
/// (e.g. the `openrouter/auto` meta-router reports `{"prompt":"-1","completion":"-1"}`).
/// A negative per-token price would otherwise propagate into the UI as a
/// nonsensical negative cost estimate, so any sub-zero value is clamped to 0.0
/// ("unknown / free") here.
fn price_per_m(pricing: &serde_json::Value, key: &str) -> f64 {
    pricing
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|per_token| *per_token >= 0.0)
        .map(|per_token| per_token * 1_000_000.0)
        .unwrap_or(0.0)
}

/// Per-model spec derived from OpenRouter's `/api/v1/models`. Mirrors the fields
/// our registry / Register-model modal care about, plus the two capability flags
/// we can infer from `supported_parameters`.
#[derive(serde::Serialize, Clone)]
pub struct OpenRouterModelSpec {
    pub id: String,
    pub name: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
    pub supports_temperature: bool,
    pub supports_reasoning_effort: bool,
}

/// The subset of an OpenRouter model spec the send path needs so cost, context
/// window, and max-output stay accurate for models that aren't in the static
/// `KNOWN_MODELS` registry. Cached process-globally by `fetch_openrouter_model_specs`.
#[derive(Clone)]
pub struct OpenRouterCost {
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
    pub context_window: u32,
    pub max_output_tokens: u32,
}

// Process-global, populated whenever `fetch_openrouter_model_specs` runs (the
// model picker calls it when an OpenRouter group is shown). The send path reads
// it via `openrouter_cost` to avoid the Sonnet-class fallback for cheap models.
static OPENROUTER_COSTS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, OpenRouterCost>>,
> = std::sync::OnceLock::new();

fn openrouter_costs(
) -> &'static std::sync::RwLock<std::collections::HashMap<String, OpenRouterCost>> {
    OPENROUTER_COSTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Look up cached OpenRouter pricing for a model id. Returns `None` until the
/// catalogue has been fetched at least once this session.
pub fn openrouter_cost(model_id: &str) -> Option<OpenRouterCost> {
    openrouter_costs().read().ok()?.get(model_id).cloned()
}

// Full-catalogue spec cache (the `/models` payload is large; refetching on every
// popover open is wasteful). 5-minute TTL like the model-list cache above.
type SpecCache = tokio::sync::Mutex<Option<(Vec<OpenRouterModelSpec>, std::time::Instant)>>;

static OR_SPEC_CACHE: std::sync::OnceLock<SpecCache> = std::sync::OnceLock::new();

fn or_spec_cache() -> &'static SpecCache {
    OR_SPEC_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Fetch per-model specs for OpenRouter's entire catalogue.
///
/// Populates the process-global cost cache as a side effect so cost estimates
/// for OpenRouter models stay accurate at send time. The `/models` endpoint is
/// public, so no API key is required.
pub async fn fetch_openrouter_model_specs(
    force_refresh: Option<bool>,
) -> Result<Vec<OpenRouterModelSpec>, String> {
    if !force_refresh.unwrap_or(false) {
        let cache = or_spec_cache().lock().await;
        if let Some((specs, fetched_at)) = cache.as_ref() {
            if fetched_at.elapsed() < MODEL_CACHE_TTL {
                return Ok(specs.clone());
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default();

    let res = client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("OpenRouter /v1/models returned {status}: {body}"));
    }
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let empty = vec![];
    let entries = json["data"].as_array().unwrap_or(&empty);

    let mut specs: Vec<OpenRouterModelSpec> = Vec::with_capacity(entries.len());
    for m in entries {
        let id = match m["id"].as_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let pricing = &m["pricing"];
        let params: Vec<&str> = m["supported_parameters"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // Cap context_window at 2M to prevent absurdly large values from meta-models.
        // Most models have 128K-1M context; 2M is a reasonable upper bound.
        let context_window = m["context_length"]
            .as_u64()
            .map(|v| std::cmp::min(v, 2_000_000))
            .unwrap_or(0) as u32;
        // `top_provider.max_completion_tokens` can be null. Fall back to a
        // sensible default (16K) instead of the full context window, which
        // would leave no space for input tokens on large-context models.
        // Most models output 4k-16k tokens max; 16K is a safe upper bound.
        // Cap at 128K to prevent the API from returning nonsensical values
        // (some meta-models like openrouter/auto report 2M, which would
        // leave zero space for input and cause condense loops).
        let max_output_tokens = m["top_provider"]["max_completion_tokens"]
            .as_u64()
            .map(|v| std::cmp::min(v, 131_072))
            .unwrap_or(16_384) as u32;

        let spec = OpenRouterModelSpec {
            id: id.clone(),
            name: m["name"].as_str().unwrap_or(&id).to_string(),
            context_window,
            max_output_tokens,
            input_cost_per_m: price_per_m(pricing, "prompt"),
            output_cost_per_m: price_per_m(pricing, "completion"),
            cache_read_cost_per_m: price_per_m(pricing, "input_cache_read"),
            cache_write_cost_per_m: price_per_m(pricing, "input_cache_write"),
            supports_temperature: params.contains(&"temperature"),
            supports_reasoning_effort: params.contains(&"reasoning"),
        };
        specs.push(spec);
    }

    // Refresh the cost cache wholesale so stale entries don't linger.
    if let Ok(mut costs) = openrouter_costs().write() {
        costs.clear();
        for s in &specs {
            costs.insert(
                s.id.clone(),
                OpenRouterCost {
                    input_cost_per_m: s.input_cost_per_m,
                    output_cost_per_m: s.output_cost_per_m,
                    cache_read_cost_per_m: s.cache_read_cost_per_m,
                    cache_write_cost_per_m: s.cache_write_cost_per_m,
                    context_window: s.context_window,
                    max_output_tokens: s.max_output_tokens,
                },
            );
        }
    }

    specs.sort_by(|a, b| a.id.cmp(&b.id));
    {
        let mut cache = or_spec_cache().lock().await;
        *cache = Some((specs.clone(), std::time::Instant::now()));
    }
    Ok(specs)
}

/// Per-provider stats for a single OpenRouter model, for the view-only provider
/// panel. `provider_name` is the join key between the two sources.
#[derive(serde::Serialize, Clone, Default)]
pub struct OpenRouterProvider {
    pub provider_name: String,
    /// Lowercase provider slug used for routing (`provider.only`). Derived from
    /// the official endpoint `tag` ("deepinfra/fp4" -> "deepinfra").
    pub provider_slug: String,
    /// Favicon-style icon URL (best-effort, from the frontend stats endpoint).
    pub icon_url: Option<String>,
    /// e.g. "fp8", "fp4". `None` when the provider reports "unknown".
    pub quantization: Option<String>,
    pub context_length: u32,
    pub max_completion_tokens: Option<u32>,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    pub cache_read_cost_per_m: f64,
    pub cache_write_cost_per_m: f64,
    /// Uptime % over the last 30 minutes (official, reliable).
    pub uptime_30m: Option<f64>,
    /// Median output throughput in tokens/sec (best-effort, frontend stats).
    pub throughput_tps: Option<f64>,
    /// Median time-to-first-token in ms (best-effort, frontend stats).
    pub latency_ms: Option<f64>,
    pub supported_parameters: Vec<String>,
}

// Per-model provider-stats cache, 5-minute TTL.
type ProviderCache = tokio::sync::Mutex<
    std::collections::HashMap<String, (Vec<OpenRouterProvider>, std::time::Instant)>,
>;

static OR_PROVIDER_CACHE: std::sync::OnceLock<ProviderCache> = std::sync::OnceLock::new();

fn or_provider_cache() -> &'static ProviderCache {
    OR_PROVIDER_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Fetch the providers serving a given OpenRouter model, with pricing/uptime
/// (official) enriched with speed/TTFT/icon (best-effort, unofficial endpoint).
pub async fn fetch_openrouter_providers(
    model_id: String,
    force_refresh: Option<bool>,
) -> Result<Vec<OpenRouterProvider>, String> {
    if !force_refresh.unwrap_or(false) {
        let cache = or_provider_cache().lock().await;
        if let Some((providers, fetched_at)) = cache.get(&model_id) {
            if fetched_at.elapsed() < MODEL_CACHE_TTL {
                return Ok(providers.clone());
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    // --- Source 1 (official, reliable): pricing, context, quantization, uptime.
    let url = format!("https://openrouter.ai/api/v1/models/{}/endpoints", model_id);
    let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("OpenRouter endpoints returned {status}: {body}"));
    }
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let empty = vec![];
    let endpoints = json["data"]["endpoints"].as_array().unwrap_or(&empty);

    // The canonical permaslug (needed for the stats endpoint) isn't a top-level
    // field, but each endpoint's `name` is "Provider | author/slug-date" — the
    // part after " | " is exactly that slug, identical across all endpoints.
    let mut permaslug: Option<String> = None;
    let mut providers: Vec<OpenRouterProvider> = Vec::with_capacity(endpoints.len());
    for ep in endpoints {
        if permaslug.is_none() {
            if let Some((_, slug)) = ep["name"].as_str().and_then(|n| n.split_once(" | ")) {
                permaslug = Some(slug.trim().to_string());
            }
        }
        let pricing = &ep["pricing"];
        let provider_name = ep["provider_name"].as_str().unwrap_or("").to_string();
        // `tag` is like "deepinfra/fp4" or "streamlake" — the segment before
        // the first '/' is the routing slug. Fall back to a lowercased name.
        let provider_slug = ep["tag"]
            .as_str()
            .map(|t| t.split('/').next().unwrap_or(t).to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider_name.to_lowercase().replace(' ', "-"));
        providers.push(OpenRouterProvider {
            provider_name,
            provider_slug,
            icon_url: None,
            quantization: ep["quantization"]
                .as_str()
                .filter(|s| *s != "unknown" && !s.is_empty())
                .map(|s| s.to_string()),
            context_length: ep["context_length"].as_u64().unwrap_or(0) as u32,
            max_completion_tokens: ep["max_completion_tokens"].as_u64().map(|n| n as u32),
            input_cost_per_m: price_per_m(pricing, "prompt"),
            output_cost_per_m: price_per_m(pricing, "completion"),
            cache_read_cost_per_m: price_per_m(pricing, "input_cache_read"),
            cache_write_cost_per_m: price_per_m(pricing, "input_cache_write"),
            uptime_30m: ep["uptime_last_30m"].as_f64(),
            throughput_tps: ep["throughput_last_30m"].as_f64(),
            latency_ms: ep["latency_last_30m"].as_f64(),
            supported_parameters: ep["supported_parameters"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        });
    }

    // --- Source 2 (unofficial, best-effort): throughput, TTFT, provider icon.
    // Any failure here is swallowed — we still return the official data.
    if let Some(slug) = permaslug {
        let furl = format!(
            "https://openrouter.ai/api/frontend/stats/endpoint?permaslug={}&variant=standard",
            slug
        );
        if let Ok(fres) = client.get(&furl).send().await {
            if fres.status().is_success() {
                if let Ok(fjson) = fres.json::<serde_json::Value>().await {
                    if let Some(arr) = fjson["data"].as_array() {
                        for item in arr {
                            let pname = item["provider_name"].as_str().unwrap_or("");
                            if let Some(p) = providers.iter_mut().find(|p| p.provider_name == pname)
                            {
                                let stats = &item["stats"];
                                if p.throughput_tps.is_none() {
                                    p.throughput_tps = stats["p50_throughput"].as_f64();
                                }
                                if p.latency_ms.is_none() {
                                    p.latency_ms = stats["p50_latency"].as_f64();
                                }
                                p.icon_url = item["provider_info"]["icon"]["url"]
                                    .as_str()
                                    .map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Fastest first (providers with no speed data sink to the bottom), then by name.
    providers.sort_by(|a, b| {
        b.throughput_tps
            .unwrap_or(0.0)
            .partial_cmp(&a.throughput_tps.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.provider_name.cmp(&b.provider_name))
    });

    {
        let mut cache = or_provider_cache().lock().await;
        cache.insert(model_id, (providers.clone(), std::time::Instant::now()));
    }
    Ok(providers)
}
