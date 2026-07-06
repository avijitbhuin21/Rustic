//! `web_search` and `web_fetch` tool definitions and client-side executors.
//!
//! Claude/Gemini/GPT-5 replace these with server-side tools; OpenAI-compatible
//! providers forward the call back here for local Tavily/Brave execution.

use crate::config::{ToolConfig, WebSearchBackend};
use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role, ToolDef};
use crate::tools::{coerce_batch_array, ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// `native_web_search` is set when the active provider runs web search itself
/// (FreeBuff/codebuff). In that case `web_search` is always offered — it needs
/// no Tavily/Brave backend or API key, and the executor routes the call to the
/// provider instead of the local search backends.
pub fn definitions_for(config: &ToolConfig, native_web_search: bool) -> Vec<ToolDef> {
    let mut defs = Vec::new();

    // Offer web_search when a native-search provider is active, or when the user
    // has enabled a local backend (Tavily/Brave; Mcp is skipped to avoid a
    // name collision with the MCP server's own web_search tool).
    if native_web_search
        || (config.web_search.enabled && config.web_search.backend != WebSearchBackend::Mcp)
    {
        defs.push(ToolDef {
            name: "web_search".to_string(),
            description:
                "Search the web for up-to-date information. Returns a list of results with \
                title, URL, and snippet. Use this when the user asks about recent events, \
                current documentation, or anything that may have changed since your knowledge \
                cutoff. Prefer focused queries; the search backend returns at most 10 results. \
                \
                BATCH MODE: pass `queries: [{query}, ...]` to run several searches in one call. \
                Mutually exclusive with the top-level `query` field. Empty array is an error."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query — phrase as a natural sentence or a set of keywords. Required in single mode; omit when using `queries`."
                    },
                    "queries": {
                        "type": "array",
                        "description": "Batch mode: run N searches in one call. Each entry has the same shape as a single-search call (`{query}`). Mutually exclusive with the top-level `query` field. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" }
                            },
                            "required": ["query"]
                        }
                    }
                }
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
                quotes or byte-level content. Public HTTPS URLs only. \
                \
                BATCH MODE: pass `fetches: [{url, prompt?}, ...]` to fetch several URLs in \
                one call. Mutually exclusive with the top-level `url`/`prompt` fields. Each \
                URL is fetched and summarised independently; empty array is an error."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Absolute HTTPS URL to fetch. Required in single mode; omit when using `fetches`."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional natural-language hint for what to extract from the page."
                    },
                    "fetches": {
                        "type": "array",
                        "description": "Batch mode: fetch N URLs in one call. Each entry uses the same shape as a single-fetch call (`{url, prompt?}`). Mutually exclusive with the top-level `url`/`prompt` fields. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "url": { "type": "string" },
                                "prompt": { "type": "string" }
                            },
                            "required": ["url"]
                        }
                    }
                }
            }),
        });
    }

    defs
}

pub async fn execute(
    name: &str,
    _tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    match name {
        "web_search" => run_web_search_dispatch(params, context).await,
        "web_fetch" => run_web_fetch_dispatch(params, context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown web tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn run_web_search_dispatch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(queries) = coerce_batch_array(params.get("queries")) {
        if params.get("query").is_some() {
            return Ok(ToolOutput {
                content: "BATCH_WEB_SEARCH_REJECTED: `queries` was provided alongside top-level \
                          `query` field. Use one shape or the other, not both."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        if queries.is_empty() {
            return Ok(ToolOutput {
                content:
                    "BATCH_WEB_SEARCH_REJECTED: `queries` array is empty. Pass at least one entry, \
                          or use the single-search shape `{ query }`."
                        .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        let mut shape_errors: Vec<String> = Vec::new();
        for (i, entry) in queries.iter().enumerate() {
            let q = entry
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if q.is_empty() {
                shape_errors.push(format!(
                    "entry[{}]: `query` is required and must be non-empty",
                    i
                ));
            }
        }
        if !shape_errors.is_empty() {
            return Ok(ToolOutput {
                content: format!(
                    "BATCH_WEB_SEARCH_REJECTED: {} entry/entries failed validation.\n{}",
                    shape_errors.len(),
                    shape_errors.join("\n"),
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        let mut out = String::new();
        let mut all_errored = true;
        for (i, entry) in queries.iter().enumerate() {
            let q_preview = entry.get("query").and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!(
                "=== web_search entry {}: \"{}\" ===\n",
                i + 1,
                q_preview
            ));
            let result = run_web_search_one(entry.clone(), context).await?;
            if !result.is_error {
                all_errored = false;
            }
            out.push_str(&result.content);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        return Ok(ToolOutput {
            content: out.trim_end().to_string(),
            is_error: all_errored,
            attachments: Vec::new(),
        });
    }
    run_web_search_one(params, context).await
}

async fn run_web_fetch_dispatch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(fetches) = coerce_batch_array(params.get("fetches")) {
        if params.get("url").is_some() || params.get("prompt").is_some() {
            return Ok(ToolOutput {
                content: "BATCH_WEB_FETCH_REJECTED: `fetches` was provided alongside top-level \
                          `url`/`prompt` fields. Use one shape or the other, not both."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        if fetches.is_empty() {
            return Ok(ToolOutput {
                content:
                    "BATCH_WEB_FETCH_REJECTED: `fetches` array is empty. Pass at least one entry, \
                          or use the single-fetch shape `{ url, prompt? }`."
                        .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        let mut shape_errors: Vec<String> = Vec::new();
        for (i, entry) in fetches.iter().enumerate() {
            let u = entry
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if u.is_empty() {
                shape_errors.push(format!(
                    "entry[{}]: `url` is required and must be non-empty",
                    i
                ));
            }
        }
        if !shape_errors.is_empty() {
            return Ok(ToolOutput {
                content: format!(
                    "BATCH_WEB_FETCH_REJECTED: {} entry/entries failed validation.\n{}",
                    shape_errors.len(),
                    shape_errors.join("\n"),
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        let mut out = String::new();
        let mut all_errored = true;
        for (i, entry) in fetches.iter().enumerate() {
            let u_preview = entry.get("url").and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!(
                "=== web_fetch entry {}: {} ===\n",
                i + 1,
                u_preview
            ));
            let result = run_web_fetch_one(entry.clone(), context).await?;
            if !result.is_error {
                all_errored = false;
            }
            out.push_str(&result.content);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        return Ok(ToolOutput {
            content: out.trim_end().to_string(),
            is_error: all_errored,
            attachments: Vec::new(),
        });
    }
    run_web_fetch_one(params, context).await
}

async fn run_web_search_one(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let query = match params.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q.trim().to_string(),
        _ => {
            return Ok(ToolOutput {
                content: "web_search requires a non-empty `query` parameter.".to_string(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let cfg = &context.tool_config.web_search;
    if !cfg.enabled {
        return Ok(ToolOutput {
            content: "web_search is disabled. Enable it in Settings → Tools.".to_string(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    if cfg.api_key.trim().is_empty() {
        return Ok(ToolOutput {
            content: "web_search backend has no API key configured. \
                Open Settings → Tools and supply one."
                .to_string(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    match cfg.backend {
        WebSearchBackend::Tavily => search_tavily(&query, &cfg.api_key).await,
        WebSearchBackend::Brave => search_brave(&query, &cfg.api_key).await,
        WebSearchBackend::Mcp => Ok(ToolOutput {
            content: "web_search is set to Tavily MCP — the MCP server handles this tool. \
                Ensure the MCP server is configured under Settings → MCP Servers."
                .to_string(),
            is_error: true,
            attachments: Vec::new(),
        }),
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

    let data: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    let results = data
        .get("results")
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
        let title = r
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("(untitled)");
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
        is_error: false,
        attachments: Vec::new(),
    })
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

    let data: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
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
        let title = r
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("(untitled)");
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
        is_error: false,
        attachments: Vec::new(),
    })
}

const MAX_FETCH_BYTES: usize = 10 * 1024 * 1024;
const MAX_MARKDOWN_CHARS: usize = 100_000;
/// 60 s keeps us below the executor's stall watchdog on a hanging host.
const FETCH_TIMEOUT_SECS: u64 = 60;

async fn run_web_fetch_one(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let url_str = match params.get("url").and_then(|v| v.as_str()) {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => {
            return Ok(ToolOutput {
                content: "web_fetch requires a non-empty `url` parameter.".to_string(),
                is_error: true,
                attachments: Vec::new(),
            });
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
            is_error: true,
            attachments: Vec::new(),
        });
    }

    // Upgrade http→https and reject IP-literal private hosts; DNS rebinding is
    // caught later by resolve_and_check after each redirect hop.
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

    // Manual redirect loop: each Location target is SSRF-checked before following,
    // preventing DNS-rebinding where the resolver returns a different IP per hop.
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
                    content: format!("web_fetch rejected URL {}: {}", current_url, msg),
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

        let port = if current_url.starts_with("https://") {
            443
        } else {
            80
        };
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
                        content: format!("web_fetch refused redirect to {}: {}", next, msg),
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

    // Reject oversized bodies before buffering: check the declared length
    // first, then read incrementally and abort as soon as the cap is exceeded
    // (never holding more than MAX_FETCH_BYTES in memory).
    if let Some(len) = final_resp.content_length() {
        if len as usize > MAX_FETCH_BYTES {
            return Ok(ToolOutput {
                content: format!(
                    "web_fetch body too large ({} bytes declared, cap {}). Refine the URL or use a direct API.",
                    len, MAX_FETCH_BYTES
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }
    let mut resp = final_resp;
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_FETCH_BYTES {
                    return Ok(ToolOutput {
                        content: format!(
                            "web_fetch body too large (over {} byte cap). Refine the URL or use a direct API.",
                            MAX_FETCH_BYTES
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("web_fetch could not read body: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
    }

    let body_text = String::from_utf8_lossy(&bytes).to_string();

    let mut markdown = strip_html(&body_text);
    if markdown.len() > MAX_MARKDOWN_CHARS {
        let mut cut = MAX_MARKDOWN_CHARS;
        while cut > 0 && !markdown.is_char_boundary(cut) {
            cut -= 1;
        }
        markdown.truncate(cut);
        markdown.push_str("\n\n[... content truncated at 100K chars]");
    }

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

/// Summarize fetched markdown via a cheap sibling model. Returns `None` to fall
/// back to raw markdown when no suitable provider is available.
async fn summarize_page(
    url: &str,
    markdown: &str,
    prompt_hint: Option<&str>,
    context: &ToolContext,
) -> Option<String> {
    let parent = context.parent_provider_config.as_ref()?;

    let small_model = small_model_for(&parent.model);
    let provider: Arc<dyn AiProvider> = if small_model.starts_with("claude-") {
        Arc::new(crate::provider::claude::ClaudeProvider::new())
    } else if small_model.starts_with("gemini-") {
        Arc::new(crate::provider::gemini::GeminiProvider::new())
    } else if small_model.starts_with("gpt-")
        || small_model.starts_with("chatgpt-")
        || (small_model.starts_with('o')
            && small_model
                .chars()
                .nth(1)
                .map_or(false, |c| c.is_ascii_digit()))
    {
        Arc::new(crate::provider::openai::OpenAiProvider::new())
    } else {
        // Unknown/custom model — route via compatible; requires parent's base_url.
        parent.base_url.as_ref()?;
        Arc::new(crate::provider::compatible::CompatibleProvider::new(
            "Compatible".to_string(),
        ))
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
        supports_adaptive_thinking: parent.supports_adaptive_thinking,
        cancel_token: context.cancel_token.clone(),
        custom_input_cost: parent.custom_input_cost,
        custom_output_cost: parent.custom_output_cost,
        custom_cache_read_cost: parent.custom_cache_read_cost,
        custom_cache_write_cost: parent.custom_cache_write_cost,
        // Internal summarizer uses its own small model; don't inherit the
        // parent's per-model provider allow-list (it may not serve this model).
        allowed_providers: None,
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

fn small_model_for(model: &str) -> String {
    let m = model.to_lowercase();
    if m.contains("claude") {
        return "claude-haiku-4-5-20251001".to_string();
    }
    if m.starts_with("gpt-")
        || m.starts_with("chatgpt-")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
    {
        return "gpt-4o-mini".to_string();
    }
    if m.starts_with("gemini") {
        return "gemini-2.5-flash-lite".to_string();
    }
    model.to_string()
}

/// Require https scheme, reject localhost and private-range IP literals (SSRF hardening).
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

    // Single canonical host parser (also used by the redirect-hop SSRF gate in
    // resolve_and_check) — strips userinfo, port, and IPv6 brackets.
    let host_lc = match host_of(&upgraded) {
        Some(h) => h.to_ascii_lowercase(),
        None => return Err("URL has no host".to_string()),
    };

    if matches!(host_lc.as_str(), "localhost") {
        return Err("refusing to fetch localhost".to_string());
    }

    if let Some(reason) = ip_literal_is_private(&host_lc) {
        return Err(format!("refusing to fetch {}: {}", host_lc, reason));
    }

    Ok(upgraded)
}

/// Returns `Some(reason)` if `host` is an IP literal in a private/reserved range.
fn ip_literal_is_private(host: &str) -> Option<&'static str> {
    let candidate = host.trim_start_matches('[').trim_end_matches(']');
    let parsed: IpAddr = candidate.parse().ok()?;
    ip_addr_is_private(&parsed)
}

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
            // 192.0.0.0/24 — IETF protocol assignments (RFC 6890).
            if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
                return Some("IETF protocol assignments");
            }
            // 198.18.0.0/15 — network benchmarking (RFC 2544).
            if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
                return Some("benchmarking range");
            }
            // 240.0.0.0/4 — reserved for future use (RFC 1112).
            if octets[0] >= 240 {
                return Some("reserved (240.0.0.0/4)");
            }
            // Explicit check for cloud metadata endpoint (also link-local, but be clear).
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
            // NAT64 well-known prefix 64:ff9b::/96 (RFC 6052): the last 32 bits
            // embed an IPv4 address — vet it like a direct IPv4 target.
            if segs[0] == 0x0064 && segs[1] == 0xff9b && segs[2..6] == [0, 0, 0, 0] {
                let embedded = std::net::Ipv4Addr::new(
                    (segs[6] >> 8) as u8,
                    (segs[6] & 0xff) as u8,
                    (segs[7] >> 8) as u8,
                    (segs[7] & 0xff) as u8,
                );
                return ip_addr_is_private(&IpAddr::V4(embedded))
                    .map(|_| "NAT64-embedded private address");
            }
            if let Some(mapped) = v6.to_ipv4() {
                return ip_addr_is_private(&IpAddr::V4(mapped));
            }
            None
        }
    }
}

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

/// Resolve `url`'s host, verify every address is public (SSRF gate), return first IP for pinning.
async fn resolve_and_check(url: &str) -> std::result::Result<IpAddr, String> {
    let host = host_of(url).ok_or_else(|| "URL has no host".to_string())?;

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
        return Some(format!("{}{}", scheme_and_host, loc));
    }
    let cur_path = match path_start {
        Some(i) => &current[i..],
        None => "/",
    };
    let cur_path = cur_path.split('?').next().unwrap_or(cur_path);
    let cur_path = cur_path.split('#').next().unwrap_or(cur_path);
    let dir_end = cur_path.rfind('/').map(|i| i + 1).unwrap_or(0);
    Some(format!(
        "{}{}{}",
        scheme_and_host,
        &cur_path[..dir_end],
        loc
    ))
}

/// Strip HTML tags, drop `<script>`/`<style>` blocks, decode common entities,
/// collapse whitespace — sufficient for prompt-grade summarization.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    // Lowercase ONCE up front for case-insensitive tag search. ASCII lowering
    // preserves byte lengths, so indices into `lower` align with `html` —
    // re-lowering the remainder on every iteration was O(n²) on script-heavy pages.
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    loop {
        let lower_rest = &lower[pos..];
        let script = lower_rest.find("<script");
        let style = lower_rest.find("<style");
        let start = match (script, style) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        match start {
            None => {
                out.push_str(&html[pos..]);
                break;
            }
            Some(s) => {
                out.push_str(&html[pos..pos + s]);
                // Find matching close tag (case-insensitive)
                let lower_tail = &lower[pos + s..];
                let close = lower_tail
                    .find("</script>")
                    .or_else(|| lower_tail.find("</style>"));
                match close {
                    None => break,
                    Some(c) => {
                        let advance = c + 9; // +9 = len of "</script>" or "</style>"
                        if pos + s + advance >= html.len() {
                            break;
                        }
                        pos += s + advance;
                    }
                }
            }
        }
    }

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

    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

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
