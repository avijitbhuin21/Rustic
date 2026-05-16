//! `web_search` and `web_fetch` tool definitions and client-side executors.
//!
//! The ToolDef signatures are shared across all providers. Provider adapters
//! decide how to honor them:
//!
//! - **Claude** / **Gemini**: the adapter replaces the function declaration
//!   with its native server-side tool spec (see `provider/claude.rs` and
//!   `provider/gemini.rs`). The server runs the tool; the executor sees a
//!   matched ToolUse+ToolResult pair in the assistant response and skips
//!   local execution.
//! - **OpenAI (GPT-5 family via Responses API)**: same pattern — the
//!   `provider/openai.rs` adapter injects `{"type":"web_search"}` as a
//!   built-in tool and synthesizes a ToolUse+ToolResult pair from the
//!   `web_search_call` items + `url_citation` annotations the API returns.
//!   No Tavily/Brave key needed; the search is billed by OpenAI.
//! - **OpenAI Chat Completions (non-GPT-5) / OpenAI-compatible /
//!   OpenRouter**: the adapter forwards the function declaration to the
//!   model as-is. When the model invokes it, the executor routes the call
//!   back here and we run the search / fetch locally using the user's
//!   configured backend (Tavily or Brave for search; reqwest + html2md +
//!   model-summarization for fetch).

use crate::config::{ToolConfig, WebSearchBackend};
use crate::provider::{
    AiProvider, ContentBlock, Message, ProviderConfig, Role, ToolDef,
};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// Builtin ToolDefs for web_search and web_fetch. Returned by
/// `BuiltinTools::definitions()` only when the corresponding flag is enabled
/// in the shared `ToolConfig`.
pub fn definitions_for(config: &ToolConfig) -> Vec<ToolDef> {
    let mut defs = Vec::new();

    // Register web_search only when enabled AND the backend is not Mcp —
    // the Mcp backend delegates to a user-configured MCP server's web_search
    // tool, so declaring our own here would collide on name.
    if config.web_search.enabled && config.web_search.backend != WebSearchBackend::Mcp {
        defs.push(ToolDef {
            name: "web_search".to_string(),
            description:
                "Search the web for up-to-date information. Returns a list of results with \
                title, URL, and snippet. Use this when the user asks about recent events, \
                current documentation, or anything that may have changed since your knowledge \
                cutoff. Prefer focused queries; the search backend returns at most 10 results."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query — phrase as a natural sentence or a set of keywords."
                    }
                },
                "required": ["query"]
            }),
        });
    }

    if config.web_fetch.enabled {
        defs.push(ToolDef {
            name: "web_fetch".to_string(),
            description:
                "Fetch a URL and return a short, prompt-focused summary of its content. Use \
                this to read documentation pages, blog posts, API references, or anything \
                where a search snippet isn't enough. The URL is downloaded, converted to \
                markdown, and summarized with a small model — do NOT rely on it for exact \
                quotes or byte-level content. Public HTTPS URLs only."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Absolute HTTPS URL to fetch."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional natural-language hint for what to extract from the page."
                    }
                },
                "required": ["url"]
            }),
        });
    }

    defs
}

/// Client-side dispatch for `web_search` / `web_fetch`. Only reachable when
/// the active provider hands the call back to us (OpenAI / OpenAI-compatible).
/// Claude and Gemini short-circuit via server-side execution.
pub async fn execute(
    name: &str,
    _tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    match name {
        "web_search" => run_web_search(params, context).await,
        "web_fetch" => run_web_fetch(params, context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown web tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

// ── web_search ───────────────────────────────────────────────────────────────

async fn run_web_search(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let query = match params.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q.trim().to_string(),
        _ => {
            return Ok(ToolOutput {
                content: "web_search requires a non-empty `query` parameter.".to_string(),
                is_error: true, attachments: Vec::new() });
        }
    };

    let cfg = &context.tool_config.web_search;
    if !cfg.enabled {
        return Ok(ToolOutput {
            content: "web_search is disabled. Enable it in Settings → Tools.".to_string(),
            is_error: true, attachments: Vec::new() });
    }
    if cfg.api_key.trim().is_empty() {
        return Ok(ToolOutput {
            content: "web_search backend has no API key configured. \
                Open Settings → Tools and supply one.".to_string(),
            is_error: true, attachments: Vec::new() });
    }

    match cfg.backend {
        WebSearchBackend::Tavily => search_tavily(&query, &cfg.api_key).await,
        WebSearchBackend::Brave => search_brave(&query, &cfg.api_key).await,
        WebSearchBackend::Mcp => Ok(ToolOutput {
            content: "web_search is set to Tavily MCP — the MCP server handles this tool. \
                Ensure the MCP server is configured under Settings → MCP Servers."
                .to_string(),
            is_error: true, attachments: Vec::new() }),
    }
}

async fn search_tavily(query: &str, api_key: &str) -> Result<ToolOutput> {
    let client = reqwest::Client::new();
    let body = json!({
        "api_key": api_key,
        "query": query,
        "max_results": 10,
        "search_depth": "basic",
        "include_answer": false,
        "include_raw_content": false,
    });
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(ToolOutput {
            content: format!("Tavily error {}: {}", status, truncate(&text, 500)),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let data: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| json!({}));
    let results = data.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    if results.is_empty() {
        return Ok(ToolOutput {
            content: format!("No results for \"{}\".", query),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let mut out = format!("Web search results for \"{}\":\n", query);
    for (i, r) in results.iter().take(10).enumerate() {
        let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("(untitled)");
        let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let snippet = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "\n{}. {} — {}\n   {}\n",
            i + 1,
            title,
            url,
            truncate(snippet, 280)
        ));
    }
    Ok(ToolOutput {
        content: out,
        is_error: false, attachments: Vec::new() })
}

async fn search_brave(query: &str, api_key: &str) -> Result<ToolOutput> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query), ("count", "10")])
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(ToolOutput {
            content: format!("Brave Search error {}: {}", status, truncate(&text, 500)),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let data: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| json!({}));
    let results = data
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if results.is_empty() {
        return Ok(ToolOutput {
            content: format!("No results for \"{}\".", query),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let mut out = format!("Web search results for \"{}\":\n", query);
    for (i, r) in results.iter().take(10).enumerate() {
        let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("(untitled)");
        let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let snippet = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "\n{}. {} — {}\n   {}\n",
            i + 1,
            title,
            url,
            truncate(snippet, 280)
        ));
    }
    Ok(ToolOutput {
        content: out,
        is_error: false, attachments: Vec::new() })
}

// ── web_fetch ────────────────────────────────────────────────────────────────

/// Hard cap on fetched body size (matches Claude Code's tool for parity).
const MAX_FETCH_BYTES: usize = 10 * 1024 * 1024; // 10 MB
/// Cap on markdown-converted body length before passing to the summarizer.
const MAX_MARKDOWN_CHARS: usize = 100_000;
/// Network timeout for the single GET. Keeps us below the executor's stall
/// watchdog in case a host hangs mid-transfer.
const FETCH_TIMEOUT_SECS: u64 = 60;

async fn run_web_fetch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let url_str = match params.get("url").and_then(|v| v.as_str()) {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => {
            return Ok(ToolOutput {
                content: "web_fetch requires a non-empty `url` parameter.".to_string(),
                is_error: true, attachments: Vec::new() });
        }
    };

    let prompt_hint = params
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if !context.tool_config.web_fetch.enabled {
        return Ok(ToolOutput {
            content: "web_fetch is disabled. Enable it in Settings → Tools.".to_string(),
            is_error: true, attachments: Vec::new() });
    }

    // URL normalization: upgrade http→https and reject IP-literal hosts that
    // already resolve to private ranges. DNS resolution + per-IP pinning
    // happens below so that a public-looking hostname which resolves to a
    // private IP (DNS rebinding, attacker-controlled DNS) is also blocked.
    let url = match normalize_url(&url_str) {
        Ok(u) => u,
        Err(msg) => {
            return Ok(ToolOutput {
                content: format!("web_fetch rejected URL: {}", msg),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    // Manual redirect loop with DNS revalidation on every hop. We disable
    // reqwest's automatic redirect handling so each Location target gets the
    // full SSRF check (host normalization + DNS lookup + IP pinning) before
    // the next request is issued. This closes the DNS-rebinding gap where the
    // first hostname resolution at SSRF-check time differs from the IP the
    // client actually connects to.
    let mut current_url = url.clone();
    let mut hops: usize = 0;
    let final_resp = loop {
        if hops >= 10 {
            return Ok(ToolOutput {
                content: format!("web_fetch refused: too many redirects from {}", url_str),
                is_error: true,
                attachments: Vec::new(),
            });
        }

        let pinned_ip = match resolve_and_check(&current_url).await {
            Ok(ip) => ip,
            Err(msg) => {
                return Ok(ToolOutput {
                    content: format!(
                        "web_fetch rejected URL {}: {}",
                        current_url, msg
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };

        let host_for_pin = match host_of(&current_url) {
            Some(h) => h,
            None => {
                return Ok(ToolOutput {
                    content: format!("web_fetch could not parse host from {}", current_url),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };

        let port = if current_url.starts_with("https://") { 443 } else { 80 };
        let pinned = SocketAddr::new(pinned_ip, port);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host_for_pin, pinned)
            .user_agent("Rustic/0.1 (+https://github.com/avijitbhuin21/Rustic)")
            .build()?;

        let resp = match client.get(&current_url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("web_fetch request failed: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };

        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let next = match location {
                Some(loc) => match resolve_redirect(&current_url, &loc) {
                    Some(u) => u,
                    None => {
                        return Ok(ToolOutput {
                            content: format!(
                                "web_fetch got redirect with un-resolvable Location {} from {}",
                                loc, current_url
                            ),
                            is_error: true,
                            attachments: Vec::new(),
                        });
                    }
                },
                None => {
                    return Ok(ToolOutput {
                        content: format!(
                            "web_fetch got redirect status {} with no Location header from {}",
                            status, current_url
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            };
            current_url = match normalize_url(&next) {
                Ok(u) => u,
                Err(msg) => {
                    return Ok(ToolOutput {
                        content: format!(
                            "web_fetch refused redirect to {}: {}",
                            next, msg
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            };
            hops += 1;
            continue;
        }

        if !status.is_success() {
            return Ok(ToolOutput {
                content: format!("web_fetch got HTTP {} from {}", status, current_url),
                is_error: true,
                attachments: Vec::new(),
            });
        }

        break resp;
    };

    let final_url = final_resp.url().to_string();
    let bytes = match final_resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("web_fetch could not read body: {}", e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    if bytes.len() > MAX_FETCH_BYTES {
        return Ok(ToolOutput {
            content: format!(
                "web_fetch body too large ({} bytes, cap {}). Refine the URL or use a direct API.",
                bytes.len(),
                MAX_FETCH_BYTES
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let body_text = String::from_utf8_lossy(&bytes).to_string();

    // HTML → markdown. Naive tag-strip used here so the crate stays
    // dependency-light; the summarization pass handles the rest.
    let mut markdown = strip_html(&body_text);
    if markdown.len() > MAX_MARKDOWN_CHARS {
        markdown.truncate(MAX_MARKDOWN_CHARS);
        markdown.push_str("\n\n[... content truncated at 100K chars]");
    }

    // Route the page through a small model for a prompt-focused summary.
    // Falls back to the raw markdown when no provider config is inherited
    // (e.g. unit tests) or the summarizer call fails.
    let summary = summarize_page(&final_url, &markdown, prompt_hint.as_deref(), context).await;
    let header = format!(
        "Fetched {} ({} bytes, {} chars after HTML strip).\n\n",
        final_url,
        bytes.len(),
        markdown.len()
    );
    match summary {
        Some(text) => Ok(ToolOutput {
            content: format!("{}{}", header, text),
            is_error: false,
            attachments: Vec::new(),
        }),
        None => Ok(ToolOutput {
            content: format!("{}{}", header, markdown),
            is_error: false,
            attachments: Vec::new(),
        }),
    }
}

/// Run the fetched markdown through a small model from the same provider
/// family as the user's current task. Returns None when we can't identify a
/// working summarization path — the caller falls back to the raw markdown so
/// the model still sees something.
async fn summarize_page(
    url: &str,
    markdown: &str,
    prompt_hint: Option<&str>,
    context: &ToolContext,
) -> Option<String> {
    let parent = context.parent_provider_config.as_ref()?;

    let small_model = small_model_for(&parent.model);
    // Pick the right provider impl by sniffing the model id. Compatible and
    // unknown models keep the parent's base_url; we assume OpenAI-compatible
    // endpoints respond to chat/completions with the same shape.
    let provider: Arc<dyn AiProvider> = if small_model.starts_with("claude-") {
        Arc::new(crate::provider::claude::ClaudeProvider::new())
    } else if small_model.starts_with("gemini-") {
        Arc::new(crate::provider::gemini::GeminiProvider::new())
    } else if small_model.starts_with("gpt-")
        || small_model.starts_with("chatgpt-")
        || (small_model.starts_with('o') && small_model.chars().nth(1).map_or(false, |c| c.is_ascii_digit()))
    {
        Arc::new(crate::provider::openai::OpenAiProvider::new())
    } else {
        // Unknown/custom model — route via compatible. Requires the parent's
        // base_url to be set; otherwise bail.
        parent.base_url.as_ref()?;
        Arc::new(crate::provider::compatible::CompatibleProvider::new("Compatible".to_string()))
    };

    let system = "You summarize a single web page for an AI coding agent. \
        Output a clean, compact markdown distillation: preserve headings, code \
        blocks, CLI flags, and URLs referenced in the page. Omit navigation, \
        ads, cookie notices, footers, and share-widget text. If a user hint is \
        given, bias the summary toward answering it. Aim for 200–800 words.";

    let hint_line = prompt_hint
        .map(|h| format!("User hint: {}\n\n", h))
        .unwrap_or_default();
    let user_prompt = format!(
        "{}URL: {}\n\nPage content (already HTML-stripped; may contain noise):\n\n{}",
        hint_line, url, markdown
    );

    let sum_config = ProviderConfig {
        api_key: parent.api_key.clone(),
        model: small_model,
        max_tokens: 4096,
        temperature: 0.0,
        base_url: parent.base_url.clone(),
        system_prompt: Some(system.to_string()),
        thinking_budget: 0,
        context_window: 0,
        web_search_enabled: false,
        web_fetch_enabled: false,
        supports_temperature: parent.supports_temperature,
        supports_reasoning_effort: parent.supports_reasoning_effort,
        cancel_token: context.cancel_token.clone(),
    };

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: user_prompt }],
    }];

    let resp = provider
        .chat(messages, Vec::new(), &sum_config, None)
        .await
        .ok()?;

    let text: String = resp
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Map the user's current model onto a cheap sibling for the fetch-summary
/// pass. Mirrors `condense::cheaper_sibling_for` but exposed here so we don't
/// have to widen its visibility or leak unrelated condensing logic.
fn small_model_for(model: &str) -> String {
    let m = model.to_lowercase();
    if m.contains("claude") {
        return "claude-haiku-4-5-20251001".to_string();
    }
    if m.starts_with("gpt-") || m.starts_with("chatgpt-") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        return "gpt-4o-mini".to_string();
    }
    if m.starts_with("gemini") {
        return "gemini-2.5-flash-lite".to_string();
    }
    // Compatible / unknown — keep the user's model, it's the best we can do.
    model.to_string()
}

/// Minimal URL validation: require a scheme, upgrade http→https, reject
/// localhost / private-range hosts (SSRF hardening). Length cap matches
/// Claude Code's tool.
fn normalize_url(raw: &str) -> std::result::Result<String, String> {
    if raw.len() > 2000 {
        return Err("URL exceeds 2000 chars".to_string());
    }
    let lowered = raw.to_ascii_lowercase();
    let upgraded = if let Some(rest) = lowered.strip_prefix("http://") {
        format!("https://{}", rest)
    } else if lowered.starts_with("https://") {
        raw.to_string()
    } else {
        return Err("URL must start with http:// or https://".to_string());
    };

    // Extract host. A parsed URL would be ideal; for a single scheme we can
    // just find the slice between "://" and the next '/', '?', or '#'.
    let after_scheme = &upgraded["https://".len()..];
    let host_end = after_scheme
        .find(|c: char| matches!(c, '/' | '?' | '#'))
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    let bare_host = host.split('@').last().unwrap_or(host); // strip userinfo
    // Strip port. IPv6 hosts arrive as "[::1]:443" — keep the '[' so the inner
    // parser still sees a valid IPv6 literal.
    let host_no_port = if bare_host.starts_with('[') {
        // Bracketed IPv6: keep up to and including the closing ']'
        match bare_host.find(']') {
            Some(end) => &bare_host[..=end],
            None => bare_host,
        }
    } else {
        bare_host.split(':').next().unwrap_or(bare_host)
    };
    let host_lc = host_no_port.to_ascii_lowercase();

    if host_lc.is_empty() {
        return Err("URL has no host".to_string());
    }

    if matches!(host_lc.as_str(), "localhost") {
        return Err("refusing to fetch localhost".to_string());
    }

    if let Some(reason) = ip_literal_is_private(&host_lc) {
        return Err(format!("refusing to fetch {}: {}", host_lc, reason));
    }

    Ok(upgraded)
}

/// If `host` is an IP literal, return Some(reason) when it falls in a
/// private / loopback / link-local / unique-local range. Returns None for
/// hostnames (DNS names) and for public IPs.
fn ip_literal_is_private(host: &str) -> Option<&'static str> {
    let candidate = host.trim_start_matches('[').trim_end_matches(']');
    let parsed: IpAddr = candidate.parse().ok()?;
    ip_addr_is_private(&parsed)
}

/// Same check, but for an already-parsed IpAddr (used after DNS resolution).
fn ip_addr_is_private(addr: &IpAddr) -> Option<&'static str> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    match addr {
        IpAddr::V4(v4) => {
            if *v4 == Ipv4Addr::UNSPECIFIED {
                return Some("unspecified address");
            }
            if v4.is_loopback() {
                return Some("loopback");
            }
            if v4.is_link_local() {
                return Some("link-local");
            }
            if v4.is_private() {
                return Some("RFC1918 private");
            }
            let octets = v4.octets();
            if octets[0] == 100 && (octets[1] >= 64 && octets[1] <= 127) {
                return Some("CGNAT");
            }
            if v4.is_broadcast() {
                return Some("broadcast");
            }
            // AWS / GCE metadata service is in the link-local range and
            // already covered, but be explicit:
            if octets == [169, 254, 169, 254] {
                return Some("cloud metadata");
            }
            None
        }
        IpAddr::V6(v6) => {
            if *v6 == Ipv6Addr::UNSPECIFIED {
                return Some("unspecified address");
            }
            if v6.is_loopback() {
                return Some("loopback");
            }
            let segs = v6.segments();
            if (segs[0] & 0xfe00) == 0xfc00 {
                return Some("unique-local");
            }
            if (segs[0] & 0xffc0) == 0xfe80 {
                return Some("link-local");
            }
            if let Some(mapped) = v6.to_ipv4() {
                return ip_addr_is_private(&IpAddr::V4(mapped));
            }
            // GCE IPv6 metadata fd00:ec2::254 falls in unique-local — already covered.
            None
        }
    }
}

/// Extract the bare host from a normalized URL (no userinfo, no port,
/// brackets stripped on IPv6). Returns None for malformed input.
fn host_of(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    let bare = host.rsplit('@').next().unwrap_or(host);
    let host_no_port = if bare.starts_with('[') {
        match bare.find(']') {
            Some(end) => &bare[1..end],
            None => bare,
        }
    } else {
        bare.split(':').next().unwrap_or(bare)
    };
    if host_no_port.is_empty() {
        None
    } else {
        Some(host_no_port.to_string())
    }
}

/// Resolve `url`'s host via DNS, verify every returned address is public, and
/// return the first address. Used as the SSRF gate plus the IP we will pin
/// the request to via `reqwest::ClientBuilder::resolve()`.
async fn resolve_and_check(url: &str) -> std::result::Result<IpAddr, String> {
    let host = host_of(url).ok_or_else(|| "URL has no host".to_string())?;

    // Already-validated by normalize_url for IP literals, but if the caller
    // passes an IP-literal host the DNS lookup will still return that IP.
    let lookup_target = format!("{}:80", host);
    let addrs = tokio::net::lookup_host(lookup_target)
        .await
        .map_err(|e| format!("DNS lookup failed for {}: {}", host, e))?;

    let mut chosen: Option<IpAddr> = None;
    for sa in addrs {
        let ip = sa.ip();
        if let Some(reason) = ip_addr_is_private(&ip) {
            return Err(format!("host {} resolves to {} ({})", host, ip, reason));
        }
        if chosen.is_none() {
            chosen = Some(ip);
        }
    }
    chosen.ok_or_else(|| format!("DNS returned no addresses for {}", host))
}

/// Resolve a Location header value against the current URL into an absolute
/// URL. Handles relative paths, absolute paths, and full URLs.
fn resolve_redirect(current: &str, location: &str) -> Option<String> {
    let loc = location.trim();
    if loc.is_empty() {
        return None;
    }
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return Some(loc.to_string());
    }
    // Strip query/fragment from current to get the path base.
    let scheme_end = current.find("://")? + 3;
    let after_scheme = &current[scheme_end..];
    let path_start = after_scheme.find('/').map(|i| scheme_end + i);
    let scheme_and_host = match path_start {
        Some(i) => &current[..i],
        None => current,
    };
    if loc.starts_with('/') {
        // Absolute path on the same host
        return Some(format!("{}{}", scheme_and_host, loc));
    }
    // Relative path — append to the current path's directory
    let cur_path = match path_start {
        Some(i) => &current[i..],
        None => "/",
    };
    let cur_path = cur_path.split('?').next().unwrap_or(cur_path);
    let cur_path = cur_path.split('#').next().unwrap_or(cur_path);
    let dir_end = cur_path.rfind('/').map(|i| i + 1).unwrap_or(0);
    Some(format!("{}{}{}", scheme_and_host, &cur_path[..dir_end], loc))
}

/// Trivial HTML → text conversion. Strips tags, decodes a handful of common
/// entities, collapses whitespace. Sufficient for a prompt-grade summary
/// without pulling in a heavyweight HTML parser crate.
fn strip_html(html: &str) -> String {
    // Kill <script> and <style> blocks entirely — their contents are never
    // useful to the model and can dwarf the real body.
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let lower = rest.to_ascii_lowercase();
        let script = lower.find("<script");
        let style = lower.find("<style");
        let start = match (script, style) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        match start {
            None => {
                out.push_str(rest);
                break;
            }
            Some(s) => {
                out.push_str(&rest[..s]);
                // Find matching close tag (case-insensitive)
                let tail = &rest[s..];
                let lower_tail = &lower[s..];
                let close = lower_tail
                    .find("</script>")
                    .or_else(|| lower_tail.find("</style>"));
                match close {
                    None => break,
                    Some(c) => {
                        // +9 for either close tag (both 9 chars including `>`)
                        let advance = c + 9;
                        if advance >= tail.len() {
                            break;
                        }
                        rest = &tail[advance..];
                    }
                }
            }
        }
    }

    // Strip remaining tags.
    let mut text = String::with_capacity(out.len());
    let mut in_tag = false;
    for ch in out.chars() {
        match (ch, in_tag) {
            ('<', _) => in_tag = true,
            ('>', true) => in_tag = false,
            (_, true) => {}
            (c, false) => text.push(c),
        }
    }

    // Decode a few common entities. A full entity table is overkill here.
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    // Collapse runs of whitespace into single spaces, preserve paragraph breaks.
    let mut collapsed = String::with_capacity(text.len());
    let mut last_was_ws = false;
    let mut blank_lines: u32 = 0;
    for ch in text.chars() {
        if ch == '\n' {
            blank_lines += 1;
            if blank_lines <= 2 {
                collapsed.push('\n');
            }
            last_was_ws = true;
        } else if ch.is_whitespace() {
            if !last_was_ws {
                collapsed.push(' ');
            }
            last_was_ws = true;
        } else {
            collapsed.push(ch);
            last_was_ws = false;
            blank_lines = 0;
        }
    }
    collapsed.trim().to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}…", &s[..end])
}
