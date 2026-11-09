//! Agent configuration + read commands (everything in `commands::agent` that is
//! NOT the chat executor / streaming path): AI provider config, model specs,
//! MCP server management, memory, budget, permissions, project defaults,
//! audio config, freebuff token management.
//!
//! Each handler reimplements the matching `#[tauri::command]` body from
//! `src-tauri/src/commands/agent/*.rs`, calling the same `rustic_agent` /
//! `rustic_db` core functions and touching the same `AppState` fields, so
//! browser behavior matches desktop exactly.
//!
//! Path mapping vs desktop:
//!   - desktop `app_data_dir(&app)`        -> `ctx.data_dir()`
//!   - user MCP json   `<app_data>/mcp.json`            -> `ctx.data_dir().join("mcp.json")`
//!   - project MCP json `<root>/.mcp.json`              -> `<project_root>/.mcp.json`
//!   - freebuff tokens `<app_data>/freebuff_tokens.json`-> `ctx.data_dir().join("freebuff_tokens.json")`
//!   - freebuff models `<app_data>/freebuff_models.json`-> `ctx.data_dir().join("freebuff_models.json")`
//!   - memory dir/index                                 -> resolved from project_root
//!
//! The MCP project-consent file is set on the manager by `rustic_app::bootstrap`
//! (`<data_dir>/mcp_consent.json`), so the gated-load path resolves identically.
//!
//! Secret mapping vs desktop: provider API keys live in the OS keychain under
//! `rustic_app::secrets::provider_account(provider_type, name)` and freebuff
//! tokens live in `<data_dir>/freebuff_tokens.json` (NOT the keychain — same as
//! desktop). We use `ctx.secrets()` with the identical account names.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rustic_agent::{
    AiConfig, LoadProjectScopeResult, McpProjectSaveResult, McpProjectServerView, McpScope,
    McpServerWithStatus, ProviderType, ToolConfig, ToolDef,
};
use rustic_app::context::AppContext;
use rustic_app::secrets::provider_account;
use rustic_app::sync_ext::MutexExt;

use crate::api::{ok, parse, project_root, ApiError, ProjectArg};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        // ── provider / config ─────────────────────────────────────────
        "get_ai_config" => get_ai_config(ctx),
        "set_ai_provider" => set_ai_provider(ctx, args),
        "remove_ai_provider" => remove_ai_provider(ctx, args),
        "get_tool_config" => get_tool_config(ctx),
        "set_tool_config" => set_tool_config(ctx, args),
        "set_subagent_config" => set_subagent_config(ctx, args),
        "clear_subagent_config" => clear_subagent_config(ctx),
        "set_model_capabilities" => set_model_capabilities(ctx, args),
        "get_model_capabilities" => get_model_capabilities(ctx),
        "set_openrouter_provider_allowlist" => set_openrouter_provider_allowlist(ctx, args),
        "get_openrouter_provider_allowlist" => get_openrouter_provider_allowlist(ctx),
        "set_permissions" => set_permissions(ctx, args),
        "set_budget_settings" => set_budget_settings(ctx, args),
        "get_budget_settings" => get_budget_settings(ctx),
        "set_subagent_concurrency_cap" => set_subagent_concurrency_cap(ctx, args),
        "set_condense_threshold" => set_condense_threshold(ctx, args),
        "get_subagent_concurrency_cap" => get_subagent_concurrency_cap(ctx),

        // ── models ─────────────────────────────────────────────────────
        "fetch_ai_models" => fetch_ai_models(ctx, args).await,
        "list_known_models" => list_known_models(),
        "fetch_openrouter_model_specs" => fetch_openrouter_model_specs(args).await,
        "fetch_openrouter_providers" => fetch_openrouter_providers(args).await,

        // ── freebuff ───────────────────────────────────────────────────
        "detect_freebuff" => detect_freebuff(),
        "freebuff_list_tokens" => freebuff_list_tokens(ctx).await,
        "freebuff_add_current_login" => freebuff_add_current_login(ctx).await,
        "freebuff_add_tokens" => freebuff_add_tokens(ctx, args).await,
        "freebuff_remove_token" => freebuff_remove_token(ctx, args).await,

        // ── mcp ────────────────────────────────────────────────────────
        "read_mcp_json" => read_mcp_json(ctx, args),
        "save_mcp_json" => save_mcp_json(ctx, args).await,
        "remove_mcp_server" => remove_mcp_server(ctx, args).await,
        "list_mcp_servers" => list_mcp_servers(ctx, args).await,
        "add_mcp_pool_server" => add_mcp_pool_server(ctx, args).await,
        "get_mcp_server_projects" => get_mcp_server_projects(ctx, args).await,
        "save_mcp_project_server" => save_mcp_project_server(ctx, args).await,
        "list_mcp_server_tools" => list_mcp_server_tools(ctx, args).await,
        "test_mcp_server" => test_mcp_server(ctx, args).await,
        "test_mcp_entry" => test_mcp_entry(args).await,
        "get_pending_mcp_consent" => get_pending_mcp_consent(ctx, args).await,
        "approve_mcp_project_consent" => approve_mcp_project_consent(ctx, args).await,
        "revoke_mcp_project_consent" => revoke_mcp_project_consent(ctx, args).await,

        // ── memory ─────────────────────────────────────────────────────
        "get_memory" => get_memory(ctx, args),
        "clear_memory" => clear_memory(ctx, args),

        // ── audio ──────────────────────────────────────────────────────
        "set_audio_input_config" => set_audio_input_config(ctx, args),
        "clear_audio_input_config" => clear_audio_input_config(ctx),
        "transcribe_audio" => transcribe_audio(ctx, args).await,
        "set_source_control_config" => set_source_control_config(ctx, args),
        "clear_source_control_config" => clear_source_control_config(ctx),
        "generate_commit_message" => generate_commit_message(ctx, args).await,

        // ── project defaults ───────────────────────────────────────────
        "get_project_defaults" => get_project_defaults(ctx, args),
        "save_project_defaults" => save_project_defaults(ctx, args),

        _ => return None,
    })
}

// ════════════════════════════════════════════════════════════════════════
// provider / config
// ════════════════════════════════════════════════════════════════════════

/// Return a redacted copy of `ai_config`: `api_key` fields become `"__STORED__"`
/// so the webview can render "Configured / Not configured" without seeing the
/// raw secret. Mirrors desktop `get_ai_config`.
fn get_ai_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let agent = ctx.state().agent.lock_safe();
    let mut config = agent.ai_config.clone();
    for entry in config.providers.iter_mut() {
        if !entry.api_key.is_empty() {
            entry.api_key = "__STORED__".to_string();
        }
    }
    ok(config)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAiProviderArg {
    provider_type: String,
    api_key: String,
    model: String,
    base_url: Option<String>,
    large_context: Option<bool>,
    custom_max_output_tokens: Option<u32>,
    custom_input_cost: Option<f64>,
    custom_output_cost: Option<f64>,
    custom_cached_input_cost: Option<f64>,
    custom_cached_output_cost: Option<f64>,
    custom_context_window: Option<u32>,
    custom_thinking_budget: Option<u32>,
    name: Option<String>,
}

/// Upsert a provider entry. Keys are written to `ctx.secrets()` under
/// `provider_account(type, name)`; the in-memory `ai_config` keeps real keys and
/// SQLite stores a redacted copy. Mirrors desktop `set_ai_provider` including
/// the F-04 fail-closed behavior on keychain failure.
fn set_ai_provider(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetAiProviderArg = parse(args)?;
    let mut agent = ctx.state().agent.lock_safe();

    let pt = match a.provider_type.as_str() {
        "Claude" => ProviderType::Claude,
        "OpenAi" => ProviderType::OpenAi,
        "Gemini" => ProviderType::Gemini,
        "Compatible" => ProviderType::Compatible,
        "OpenRouter" => ProviderType::OpenRouter,
        "FreeBuff" => ProviderType::FreeBuff,
        _ => {
            return Err(ApiError::from(format!(
                "Unknown provider type: {}",
                a.provider_type
            )))
        }
    };

    let base_url = if matches!(pt, ProviderType::OpenRouter) {
        Some("https://openrouter.ai/api/v1".to_string())
    } else {
        a.base_url
    };

    if let Some(ref u) = base_url {
        rustic_agent::provider::validate_provider_base_url(u)
            .map_err(|e| format!("Invalid base_url: {}", e))?;
    }

    let entry_name: Option<String> = if matches!(pt, ProviderType::Compatible) {
        a.name
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    let target_key: Option<String> = if matches!(pt, ProviderType::Compatible) {
        let probe = rustic_agent::ProviderEntry {
            provider_type: ProviderType::Compatible,
            api_key: String::new(),
            default_model: String::new(),
            base_url: None,
            enabled: false,
            large_context: false,
            custom_max_output_tokens: 0,
            custom_input_cost: 0.0,
            custom_output_cost: 0.0,
            custom_cached_input_cost: 0.0,
            custom_cached_output_cost: 0.0,
            custom_context_window: 0,
            custom_thinking_budget: 0,
            name: entry_name.clone(),
        };
        Some(probe.provider_key())
    } else {
        None
    };

    let matches_idx = agent.ai_config.providers.iter().position(|p| {
        if p.provider_type != pt {
            return false;
        }
        match &pt {
            ProviderType::Compatible => match &target_key {
                Some(k) => p.provider_key() == *k,
                None => p.name.is_none(),
            },
            _ => true,
        }
    });

    if let Some(idx) = matches_idx {
        let entry = &mut agent.ai_config.providers[idx];
        if a.api_key != "__STORED__" {
            entry.api_key = a.api_key;
        }
        entry.default_model = a.model;
        entry.base_url = base_url;
        entry.enabled = true;
        entry.name = entry_name;
        if let Some(lc) = a.large_context {
            entry.large_context = lc;
        }
        if let Some(v) = a.custom_max_output_tokens {
            entry.custom_max_output_tokens = v;
        }
        if let Some(v) = a.custom_input_cost {
            entry.custom_input_cost = v;
        }
        if let Some(v) = a.custom_output_cost {
            entry.custom_output_cost = v;
        }
        if let Some(v) = a.custom_cached_input_cost {
            entry.custom_cached_input_cost = v;
        }
        if let Some(v) = a.custom_cached_output_cost {
            entry.custom_cached_output_cost = v;
        }
        if let Some(v) = a.custom_context_window {
            entry.custom_context_window = v;
        }
        if let Some(v) = a.custom_thinking_budget {
            entry.custom_thinking_budget = v;
        }
    } else {
        agent.ai_config.providers.push(rustic_agent::ProviderEntry {
            provider_type: pt.clone(),
            api_key: a.api_key,
            default_model: a.model,
            base_url,
            enabled: true,
            large_context: a.large_context.unwrap_or(false),
            custom_max_output_tokens: a.custom_max_output_tokens.unwrap_or(0),
            custom_input_cost: a.custom_input_cost.unwrap_or(0.0),
            custom_output_cost: a.custom_output_cost.unwrap_or(0.0),
            custom_cached_input_cost: a.custom_cached_input_cost.unwrap_or(0.0),
            custom_cached_output_cost: a.custom_cached_output_cost.unwrap_or(0.0),
            custom_context_window: a.custom_context_window.unwrap_or(0),
            custom_thinking_budget: a.custom_thinking_budget.unwrap_or(0),
            name: entry_name,
        });
    }

    if agent.ai_config.default_provider.is_none() {
        agent.ai_config.default_provider = Some(pt);
    }

    // Persist: keys -> keychain (fail closed, F-04), redacted config -> SQLite.
    let mut redacted = agent.ai_config.clone();
    for entry in redacted.providers.iter_mut() {
        if entry.api_key.is_empty() {
            continue;
        }
        let provider_str = entry.provider_type.as_str();
        let acct = provider_account(provider_str, entry.name.as_deref());
        match ctx.secrets().set(&acct, &entry.api_key) {
            Ok(()) => {
                entry.api_key.clear();
            }
            Err(e) => {
                return Err(ApiError::from(format!(
                    "Could not save the API key to your secret store ({}): {}. \
                     The key was NOT written to disk in plaintext.",
                    acct, e
                )));
            }
        }
    }
    let config_json = serde_json::to_string(&redacted).map_err(|e| e.to_string())?;
    drop(agent);
    let db = ctx.state().db.lock_safe();
    db.set_setting("ai_config", &config_json)
        .map_err(|e| e.to_string())?;

    ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderKeyArg {
    provider_key: String,
}

/// Remove a provider entry + its keychain secret. Mirrors desktop
/// `remove_ai_provider`.
fn remove_ai_provider(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProviderKeyArg = parse(args)?;
    let mut agent = ctx.state().agent.lock_safe();

    let removed_account: Option<String> = agent
        .ai_config
        .providers
        .iter()
        .find(|p| p.provider_key() == a.provider_key)
        .map(|entry| {
            let provider_str = entry.provider_type.as_str();
            provider_account(provider_str, entry.name.as_deref())
        });

    let before = agent.ai_config.providers.len();
    agent
        .ai_config
        .providers
        .retain(|p| p.provider_key() != a.provider_key);

    if agent.ai_config.providers.len() == before {
        return Err(ApiError::from(format!(
            "Provider not found: {}",
            a.provider_key
        )));
    }

    if let Some(acct) = removed_account {
        if let Err(e) = ctx.secrets().delete(&acct) {
            tracing::warn!(account = %acct, error = %e, "keychain cleanup failed");
        }
    }

    if let Some(ref def) = agent.ai_config.default_provider {
        let still_present = agent
            .ai_config
            .providers
            .iter()
            .any(|p| &p.provider_type == def);
        if !still_present {
            agent.ai_config.default_provider = agent
                .ai_config
                .providers
                .first()
                .map(|p| p.provider_type.clone());
        }
    }

    let config_json = serde_json::to_string(&agent.ai_config).map_err(|e| e.to_string())?;
    drop(agent);
    let db = ctx.state().db.lock_safe();
    db.set_setting("ai_config", &config_json)
        .map_err(|e| e.to_string())?;
    ok(())
}

fn get_tool_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let agent = ctx.state().agent.lock_safe();
    ok(agent.tool_config.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolConfigArg {
    config: ToolConfig,
}

fn set_tool_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ToolConfigArg = parse(args)?;
    let mut agent = ctx.state().agent.lock_safe();
    agent.tool_config = a.config;
    let json = serde_json::to_string(&agent.tool_config).map_err(|e| e.to_string())?;
    drop(agent);
    let db = ctx.state().db.lock_safe();
    db.set_setting("tool_config", &json)
        .map_err(|e| e.to_string())?;
    ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentConfigArg {
    provider_key: String,
    model: String,
}

fn set_subagent_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SubagentConfigArg = parse(args)?;
    let provider_key = a.provider_key.trim().to_string();
    let model = a.model.trim().to_string();
    if provider_key.is_empty() || model.is_empty() {
        return Err(ApiError::from("provider_key and model are required"));
    }
    let mut agent = ctx.state().agent.lock_safe();
    if agent.ai_config.find_by_key(&provider_key).is_none() {
        return Err(ApiError::from(format!(
            "No configured provider matches key \"{}\". Pick a model from a \
             provider that's already connected.",
            provider_key
        )));
    }
    agent.ai_config.subagent = Some(rustic_agent::SubagentConfig {
        provider_key,
        model,
    });
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn clear_subagent_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let mut agent = ctx.state().agent.lock_safe();
    agent.ai_config.subagent = None;
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModelCapabilitiesArg {
    model_id: String,
    supports_temperature: Option<bool>,
    supports_reasoning_effort: Option<bool>,
    supports_adaptive_thinking: Option<bool>,
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
    request_params: Option<rustic_agent::RequestParamOverrides>,
}

fn set_model_capabilities(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetModelCapabilitiesArg = parse(args)?;
    if a.model_id.trim().is_empty() {
        return Err(ApiError::from("model_id is required"));
    }
    let mut agent = ctx.state().agent.lock_safe();
    if a.supports_temperature.is_none()
        && a.supports_reasoning_effort.is_none()
        && a.supports_adaptive_thinking.is_none()
        && a.context_window.is_none()
        && a.max_output_tokens.is_none()
        && a.request_params.is_none()
    {
        agent.ai_config.model_capabilities.remove(&a.model_id);
    } else {
        let entry = agent
            .ai_config
            .model_capabilities
            .entry(a.model_id.clone())
            .or_default();
        if let Some(v) = a.supports_temperature {
            entry.supports_temperature = v;
        }
        if let Some(v) = a.supports_reasoning_effort {
            entry.supports_reasoning_effort = v;
        }
        if let Some(v) = a.supports_adaptive_thinking {
            entry.supports_adaptive_thinking = v;
        }
        if let Some(v) = a.context_window {
            entry.context_window = v;
        }
        if let Some(v) = a.max_output_tokens {
            entry.max_output_tokens = v;
        }
        if let Some(v) = a.request_params {
            entry.request_params = v;
        }
    }
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn get_model_capabilities(ctx: &ServerContext) -> Result<Value, ApiError> {
    let agent = ctx.state().agent.lock_safe();
    ok(agent.ai_config.model_capabilities.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAllowlistArg {
    model_id: String,
    providers: Vec<String>,
}

fn set_openrouter_provider_allowlist(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetAllowlistArg = parse(args)?;
    if a.model_id.trim().is_empty() {
        return Err(ApiError::from("model_id is required"));
    }
    let mut agent = ctx.state().agent.lock_safe();
    if a.providers.is_empty() {
        agent
            .ai_config
            .openrouter_provider_allowlist
            .remove(&a.model_id);
    } else {
        agent
            .ai_config
            .openrouter_provider_allowlist
            .insert(a.model_id.clone(), a.providers);
    }
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn get_openrouter_provider_allowlist(ctx: &ServerContext) -> Result<Value, ApiError> {
    let agent = ctx.state().agent.lock_safe();
    ok(agent.ai_config.openrouter_provider_allowlist.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPermissionsArg {
    project_id: Option<String>,
    level: String,
}

fn set_permissions(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetPermissionsArg = parse(args)?;
    let perm = parse_permission_level(&a.level)?;
    let mut agent = ctx.state().agent.lock_safe();
    if let Some(pid) = a.project_id {
        agent.project_permissions.insert(pid, perm);
    }
    ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetBudgetArg {
    max_concurrent_streams: Option<usize>,
    daily_cost_ceiling_cents: Option<u64>,
    #[serde(default)]
    soft_turn_limit: Option<u32>,
}

fn set_budget_settings(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetBudgetArg = parse(args)?;
    let mut agent = ctx.state().agent.lock_safe();
    let preserved_subagents = agent.ai_config.budget.max_concurrent_subagents;
    agent.ai_config.budget = rustic_agent::budget::BudgetSettings {
        max_concurrent_streams: a.max_concurrent_streams,
        daily_cost_ceiling_cents: a.daily_cost_ceiling_cents,
        max_concurrent_subagents: preserved_subagents,
        soft_turn_limit: a.soft_turn_limit,
    };
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn get_budget_settings(ctx: &ServerContext) -> Result<Value, ApiError> {
    let agent = ctx.state().agent.lock_safe();
    ok(agent.ai_config.budget.clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapArg {
    cap: Option<usize>,
}

fn set_subagent_concurrency_cap(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: CapArg = parse(args)?;
    let mut agent = ctx.state().agent.lock_safe();
    agent.ai_config.budget.max_concurrent_subagents = a.cap;
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn get_subagent_concurrency_cap(ctx: &ServerContext) -> Result<Value, ApiError> {
    let agent = ctx.state().agent.lock_safe();
    ok(agent.ai_config.budget.max_concurrent_subagents)
}

#[derive(Deserialize)]
struct CondenseThresholdArg {
    tokens: u32,
}

/// Set the global "auto-condense at" token cap (0 = automatic). Mirrors desktop.
fn set_condense_threshold(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: CondenseThresholdArg = parse(args)?;
    let mut agent = ctx.state().agent.lock_safe();
    agent.ai_config.condense_threshold = a.tokens;
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

/// Persist `ai_config` to SQLite with API keys redacted (keychain owns the real
/// secrets). Mirrors desktop `persist_ai_config`.
fn persist_ai_config(ctx: &ServerContext, cfg: &AiConfig) -> Result<(), ApiError> {
    let mut redacted = cfg.clone();
    for entry in redacted.providers.iter_mut() {
        entry.api_key.clear();
    }
    let json = serde_json::to_string(&redacted).map_err(|e| e.to_string())?;
    let db = ctx.state().db.lock_safe();
    db.set_setting("ai_config", &json)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn parse_permission_level(level: &str) -> Result<rustic_agent::PermissionLevel, ApiError> {
    use rustic_agent::PermissionLevel;
    match level {
        "Chat" => Ok(PermissionLevel::Chat),
        "ManualEdit" => Ok(PermissionLevel::ManualEdit),
        "AutoEdit" => Ok(PermissionLevel::AutoEdit),
        "FullAuto" => Ok(PermissionLevel::FullAuto),
        _ => Err(ApiError::from(format!(
            "Unknown permission level: {}. Valid values: Chat, ManualEdit, AutoEdit, FullAuto",
            level
        ))),
    }
}

// ════════════════════════════════════════════════════════════════════════
// models  (reqwest-backed model-list fetching, cached per-process)
// ════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchModelsArg {
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
    include_all: Option<bool>,
    capability: Option<String>,
}

async fn fetch_ai_models(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: FetchModelsArg = parse(args)?;
    let freebuff_cache = Some(rustic_app::host_ops::models::freebuff_cache_path(
        &ctx.data_dir(),
    ));
    let models = rustic_app::host_ops::models::fetch_ai_models(
        ctx.state(),
        freebuff_cache,
        a.provider_type,
        a.api_key,
        a.base_url,
        a.force_refresh,
        a.include_all,
        a.capability,
    )
    .await
    .map_err(ApiError::from)?;
    ok(models)
}

// NOTE: the model-spec / provider / token / defaults return structs below
// intentionally have NO `rename_all` — the desktop equivalents serialize their
// fields as snake_case, and the frontend reads `m.max_output_tokens`,
// `p.input_cost_per_m`, `t.is_default`, etc. ProjectDefaults must also stay
// snake_case so its on-disk `settings_json` round-trips with desktop's reader.
#[derive(serde::Serialize, Clone)]
struct KnownModelOut {
    id: String,
    name: String,
    provider: String,
    max_output_tokens: u32,
    context_window: u32,
    input_cost_per_m: f64,
    output_cost_per_m: f64,
    cache_read_cost_per_m: f64,
    cache_write_cost_per_m: f64,
}

fn list_known_models() -> Result<Value, ApiError> {
    let out: Vec<KnownModelOut> = rustic_agent::model_registry::KNOWN_MODELS
        .iter()
        .map(|m| KnownModelOut {
            id: m.id.to_string(),
            name: m.name.to_string(),
            provider: m.provider.to_string(),
            max_output_tokens: m.max_output_tokens,
            context_window: m.context_window,
            input_cost_per_m: m.input_cost_per_m,
            output_cost_per_m: m.output_cost_per_m,
            cache_read_cost_per_m: m.cache_read_cost_per_m,
            cache_write_cost_per_m: m.cache_write_cost_per_m,
        })
        .collect();
    ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForceRefreshArg {
    force_refresh: Option<bool>,
}

async fn fetch_openrouter_model_specs(args: &Value) -> Result<Value, ApiError> {
    let a: ForceRefreshArg = parse(args)?;
    let specs = rustic_app::host_ops::openrouter::fetch_openrouter_model_specs(a.force_refresh)
        .await
        .map_err(ApiError::from)?;
    ok(specs)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvidersArg {
    model_id: String,
    force_refresh: Option<bool>,
}

async fn fetch_openrouter_providers(args: &Value) -> Result<Value, ApiError> {
    let a: ProvidersArg = parse(args)?;
    let providers =
        rustic_app::host_ops::openrouter::fetch_openrouter_providers(a.model_id, a.force_refresh)
            .await
            .map_err(ApiError::from)?;
    ok(providers)
}

// ════════════════════════════════════════════════════════════════════════
// freebuff  (token pool persisted to <data_dir>/freebuff_tokens.json)
// ════════════════════════════════════════════════════════════════════════

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct StoredFbToken {
    token: String,
    #[serde(default)]
    email: Option<String>,
}

#[derive(serde::Serialize)]
struct FreebuffTokenInfo {
    id: String,
    email: Option<String>,
    valid: bool,
    is_default: bool,
}

fn mask_fb_token(t: &str) -> String {
    let n = t.len();
    if n <= 10 {
        return "•".repeat(n);
    }
    format!("{}…{}", &t[..6], &t[n - 4..])
}

fn fb_tokens_path(ctx: &ServerContext) -> PathBuf {
    ctx.data_dir().join("freebuff_tokens.json")
}

fn load_fb_tokens(ctx: &ServerContext) -> Vec<StoredFbToken> {
    std::fs::read_to_string(fb_tokens_path(ctx))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_fb_tokens(ctx: &ServerContext, store: &[StoredFbToken]) -> Result<(), ApiError> {
    let json = serde_json::to_string(store).map_err(|e| e.to_string())?;
    std::fs::write(fb_tokens_path(ctx), json).map_err(|e| e.to_string())?;
    Ok(())
}

fn push_fb_pool(store: &[StoredFbToken]) {
    let tokens = store.iter().map(|s| s.token.clone()).collect();
    rustic_agent::provider::freebuff::set_pool_tokens(tokens);
}

async fn list_fb_tokens_inner(store: Vec<StoredFbToken>) -> Result<Value, ApiError> {
    let default_token = rustic_agent::provider::freebuff::read_account()
        .ok()
        .map(|(t, _)| t);
    let mut out = Vec::with_capacity(store.len());
    for s in &store {
        let valid = rustic_agent::provider::freebuff::validate_token(&s.token).await;
        out.push(FreebuffTokenInfo {
            id: mask_fb_token(&s.token),
            email: s.email.clone(),
            valid,
            is_default: default_token.as_deref() == Some(s.token.as_str()),
        });
    }
    ok(out)
}

fn detect_freebuff() -> Result<Value, ApiError> {
    ok(rustic_agent::provider::freebuff::detect())
}

async fn freebuff_list_tokens(ctx: &ServerContext) -> Result<Value, ApiError> {
    let store = load_fb_tokens(ctx);
    push_fb_pool(&store);
    list_fb_tokens_inner(store).await
}

async fn freebuff_add_current_login(ctx: &ServerContext) -> Result<Value, ApiError> {
    let (token, email) =
        rustic_agent::provider::freebuff::read_account().map_err(|e| e.to_string())?;
    let mut store = load_fb_tokens(ctx);
    if let Some(existing) = store.iter_mut().find(|s| s.token == token) {
        existing.email = email;
    } else {
        store.push(StoredFbToken { token, email });
    }
    save_fb_tokens(ctx, &store)?;
    push_fb_pool(&store);
    list_fb_tokens_inner(store).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawArg {
    raw: String,
}

async fn freebuff_add_tokens(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: RawArg = parse(args)?;
    let incoming: Vec<String> = a
        .raw
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if incoming.is_empty() {
        return Err(ApiError::from("no tokens found in the pasted text"));
    }
    let mut store = load_fb_tokens(ctx);
    for tok in incoming {
        if !store.iter().any(|s| s.token == tok) {
            store.push(StoredFbToken {
                token: tok,
                email: None,
            });
        }
    }
    save_fb_tokens(ctx, &store)?;
    push_fb_pool(&store);
    list_fb_tokens_inner(store).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdArg {
    id: String,
}

async fn freebuff_remove_token(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let mut store = load_fb_tokens(ctx);
    store.retain(|s| mask_fb_token(&s.token) != a.id);
    save_fb_tokens(ctx, &store)?;
    push_fb_pool(&store);
    list_fb_tokens_inner(store).await
}

// ════════════════════════════════════════════════════════════════════════
// MCP  (servers live in JSON files: user <data_dir>/mcp.json, project .mcp.json)
// ════════════════════════════════════════════════════════════════════════

fn parse_scope(s: &str) -> Result<McpScope, ApiError> {
    match s {
        "user" => Ok(McpScope::User),
        "project" => Ok(McpScope::Project),
        other => Err(ApiError::from(format!(
            "Unknown MCP scope: {}. Valid values: user, project",
            other
        ))),
    }
}

/// Resolve the on-disk path for a scope. User scope uses the data dir; project
/// scope resolves the project root from the workspace.
fn resolve_scope_path(
    ctx: &ServerContext,
    scope: McpScope,
    project_id: Option<&str>,
) -> Result<PathBuf, ApiError> {
    match scope {
        McpScope::User => Ok(ctx.data_dir().join("mcp.json")),
        McpScope::Project => {
            let pid = project_id
                .ok_or_else(|| ApiError::from("project_id is required for project scope"))?;
            let root = project_root(ctx, pid)?;
            Ok(PathBuf::from(root).join(".mcp.json"))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadMcpJsonArg {
    scope: String,
    project_id: Option<String>,
}

fn read_mcp_json(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ReadMcpJsonArg = parse(args)?;
    let scope = parse_scope(&a.scope)?;
    let path = resolve_scope_path(ctx, scope, a.project_id.as_deref())?;
    if path.exists() {
        ok(std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
    } else {
        ok("{\n  \"mcpServers\": {}\n}\n".to_string())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpSaveResult {
    name: String,
    connected: bool,
    tool_count: usize,
    error: Option<String>,
}

impl From<rustic_agent::McpConnectResult> for McpSaveResult {
    fn from(r: rustic_agent::McpConnectResult) -> Self {
        Self {
            name: r.name,
            connected: r.connected,
            tool_count: r.tool_count,
            error: r.error,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveMcpJsonArg {
    scope: String,
    project_id: Option<String>,
    content: String,
}

async fn save_mcp_json(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SaveMcpJsonArg = parse(args)?;
    let scope = parse_scope(&a.scope)?;
    let path = resolve_scope_path(ctx, scope, a.project_id.as_deref())?;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let content = a.content;

    let res = tokio::task::spawn_blocking(move || -> Result<Vec<McpSaveResult>, String> {
        let mut mcp = mcp_arc.lock_safe();
        match scope {
            McpScope::User => mcp.set_user_path(path.clone()),
            McpScope::Project => mcp.set_project_path(path.clone()),
        }
        mcp.save_scope_raw(scope, &content)
            .map_err(|e| e.to_string())?;
        Ok(mcp
            .test_scope(scope)
            .into_iter()
            .map(McpSaveResult::from)
            .collect())
    })
    .await
    .map_err(|e| format!("save_mcp_json task panicked: {}", e))??;
    ok(res)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMcpServersArg {
    project_id: Option<String>,
}

async fn list_mcp_servers(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ListMcpServersArg = parse(args)?;
    let user_path = resolve_scope_path(ctx, McpScope::User, None).ok();
    let project_path = a
        .project_id
        .as_deref()
        .and_then(|pid| resolve_scope_path(ctx, McpScope::Project, Some(pid)).ok());
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);

    let pool_only = a.project_id.is_none();
    let res = tokio::task::spawn_blocking(move || -> Vec<McpServerWithStatus> {
        let mut mcp = mcp_arc.lock_safe();
        if let Some(p) = user_path {
            let _ = mcp.load_scope(McpScope::User, &p);
        }
        if let Some(p) = project_path {
            let _ = mcp.load_project_scope_gated(&p);
        }
        let _ = mcp.connect_all();
        if pool_only {
            mcp.list_pool_servers_with_status()
        } else {
            mcp.list_servers_with_status()
        }
    })
    .await
    .map_err(|e| format!("list_mcp_servers task panicked: {}", e))?;
    ok(res)
}

async fn get_pending_mcp_consent(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let path = resolve_scope_path(ctx, McpScope::Project, Some(&a.project_id))?;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Result<Option<serde_json::Value>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let hash = rustic_agent::mcp_sha256_hex(text.as_bytes());
        let mcp = mcp_arc.lock_safe();
        if mcp.is_project_consented(&path, &hash) {
            return Ok(None);
        }
        Ok(Some(json!({
            "projectPath": path.display().to_string(),
            "contentHash": hash,
            "content": text,
        })))
    })
    .await
    .map_err(|e| format!("get_pending_mcp_consent task panicked: {}", e))??;
    ok(res)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveConsentArg {
    project_id: String,
    content_hash: String,
}

async fn approve_mcp_project_consent(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ApproveConsentArg = parse(args)?;
    let path = resolve_scope_path(ctx, McpScope::Project, Some(&a.project_id))?;
    let content_hash = a.content_hash;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Result<Vec<McpSaveResult>, String> {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let actual = rustic_agent::mcp_sha256_hex(text.as_bytes());
        if actual != content_hash {
            return Err(format!(
                ".mcp.json changed since the consent modal opened (expected {}, got {}); refusing to approve.",
                content_hash, actual
            ));
        }
        let mut mcp = mcp_arc.lock_safe();
        mcp.approve_project_consent(path.clone(), actual)
            .map_err(|e| e.to_string())?;
        match mcp.load_project_scope_gated(&path) {
            Ok(LoadProjectScopeResult::Loaded(_)) => {}
            Ok(other) => {
                return Err(format!("Unexpected gated load result after approval: {:?}", other));
            }
            Err(e) => return Err(e.to_string()),
        }
        Ok(mcp
            .test_scope(McpScope::Project)
            .into_iter()
            .map(McpSaveResult::from)
            .collect())
    })
    .await
    .map_err(|e| format!("approve_mcp_project_consent task panicked: {}", e))??;
    ok(res)
}

async fn revoke_mcp_project_consent(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let path = resolve_scope_path(ctx, McpScope::Project, Some(&a.project_id))?;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut mcp = mcp_arc.lock_safe();
        mcp.revoke_project_consent(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("revoke_mcp_project_consent task panicked: {}", e))??;
    ok(())
}

async fn test_mcp_server(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let id = a.id;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Result<Vec<ToolDef>, String> {
        mcp_arc
            .lock_safe()
            .test_server(&id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("test_mcp_server task panicked: {}", e))??;
    ok(res)
}

#[derive(serde::Deserialize)]
struct TestEntryArgs {
    name: String,
    entry: Value,
}

/// Connect to a not-yet-saved server entry (standard MCP JSON), list its
/// tools, then disconnect — the Add MCP server dialog's Test button.
async fn test_mcp_entry(args: &Value) -> Result<Value, ApiError> {
    let a: TestEntryArgs = parse(args)?;
    let res = tokio::task::spawn_blocking(move || -> Result<Vec<ToolDef>, String> {
        let transport =
            rustic_agent::mcp::entry_transport(&a.name, &a.entry).map_err(|e| e.to_string())?;
        let config = rustic_agent::mcp::ServerConfig {
            id: format!("test-{}", a.name),
            name: a.name.clone(),
            transport,
            enabled: true,
            scope: rustic_agent::mcp::McpScope::User,
        };
        let mut client =
            rustic_agent::mcp::client::McpClient::connect(config).map_err(|e| e.to_string())?;
        let tools = client.list_tools().map_err(|e| e.to_string());
        client.disconnect();
        tools
    })
    .await
    .map_err(|e| format!("test_mcp_entry task panicked: {}", e))??;
    ok(res)
}

async fn list_mcp_server_tools(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let id = a.id;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Result<Vec<ToolDef>, String> {
        mcp_arc
            .lock_safe()
            .server_tools(&id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_mcp_server_tools task panicked: {}", e))??;
    ok(res)
}

async fn remove_mcp_server(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let id = a.id;
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        mcp_arc
            .lock_safe()
            .remove_server(&id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("remove_mcp_server task panicked: {}", e))??;
    ok(())
}

// ════════════════════════════════════════════════════════════════════════
// memory  (.rustic/memory/ folder + MEMORY.md index inside the project root)
// ════════════════════════════════════════════════════════════════════════

fn memory_dir(project_root: &Path) -> PathBuf {
    project_root.join(".rustic").join("memory")
}

fn memory_index_path(project_root: &Path) -> PathBuf {
    memory_dir(project_root).join("MEMORY.md")
}

fn legacy_memory_path(project_root: &Path) -> PathBuf {
    project_root.join(".rustic").join("memory.md")
}

fn get_memory(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = PathBuf::from(project_root(ctx, &a.project_id)?);
    let dir = memory_dir(&root);

    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    let index_path = memory_index_path(&root);
    if !index_path.exists() {
        let seed = std::fs::read_to_string(legacy_memory_path(&root)).unwrap_or_default();
        let body = if seed.trim().is_empty() {
            "# Memory Index\n".to_string()
        } else {
            format!("# Memory Index\n\n{}\n", seed.trim())
        };
        let _ = rustic_core::io_util::atomic_write(&index_path, body.as_bytes());
    }

    let mut out = String::new();
    if let Ok(idx) = std::fs::read_to_string(&index_path) {
        out.push_str(idx.trim_end());
    }

    let mut fragments: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().and_then(|e| e.to_str()) == Some("md")
                        && p.file_name().and_then(|n| n.to_str()) != Some("MEMORY.md")
                })
                .collect()
        })
        .unwrap_or_default();
    fragments.sort();
    for frag in fragments {
        if let Ok(content) = std::fs::read_to_string(&frag) {
            let name = frag
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("fragment.md");
            out.push_str(&format!("\n\n---\n## {}\n\n{}", name, content.trim()));
        }
    }
    ok(out)
}

fn clear_memory(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = PathBuf::from(project_root(ctx, &a.project_id)?);
    let dir = memory_dir(&root);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    rustic_core::io_util::atomic_write(memory_index_path(&root).as_path(), b"# Memory Index\n")
        .map_err(|e| e.to_string())?;
    let legacy = legacy_memory_path(&root);
    if legacy.exists() {
        let _ = std::fs::remove_file(&legacy);
    }
    ok(())
}

// ════════════════════════════════════════════════════════════════════════
// audio  (speech-to-text; receives base64 audio + mime, POSTs to an STT API)
//
// Headless-compatible: the clip arrives as a base64 arg (the browser records it
// the same way the desktop webview does). Partial transcript text is streamed to
// the frontend via the same `audio-transcript-delta` / `audio-transcript-done`
// events, emitted through `ctx.emit_json` (the server's EventHub → WS).
// ════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAudioInputArg {
    provider_key: String,
    model: String,
}

fn set_audio_input_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetAudioInputArg = parse(args)?;
    let provider_key = a.provider_key.trim().to_string();
    let model = a.model.trim().to_string();
    if provider_key.is_empty() || model.is_empty() {
        return Err(ApiError::from("provider_key and model are required"));
    }
    let mut agent = ctx.state().agent.lock_safe();
    if agent.ai_config.find_by_key(&provider_key).is_none() {
        return Err(ApiError::from(format!(
            "No configured provider matches key \"{}\". Pick a model from a \
             provider that's already connected.",
            provider_key
        )));
    }
    agent.ai_config.audio_input = Some(rustic_agent::AudioInputConfig {
        provider_key,
        model,
    });
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn clear_audio_input_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let mut agent = ctx.state().agent.lock_safe();
    agent.ai_config.audio_input = None;
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn set_source_control_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetAudioInputArg = parse(args)?;
    let provider_key = a.provider_key.trim().to_string();
    let model = a.model.trim().to_string();
    if provider_key.is_empty() || model.is_empty() {
        return Err(ApiError::from("provider_key and model are required"));
    }
    let mut agent = ctx.state().agent.lock_safe();
    if agent.ai_config.find_by_key(&provider_key).is_none() {
        return Err(ApiError::from(format!(
            "No configured provider matches key \"{}\". Pick a model from a \
             provider that's already connected.",
            provider_key
        )));
    }
    agent.ai_config.source_control = Some(rustic_agent::SourceControlConfig {
        provider_key,
        model,
    });
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

fn clear_source_control_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let mut agent = ctx.state().agent.lock_safe();
    agent.ai_config.source_control = None;
    persist_ai_config(ctx, &agent.ai_config)?;
    ok(())
}

async fn generate_commit_message(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    use rustic_agent::commit_message::CommitMessageRequest;

    let a: ProjectArg = parse(args)?;

    let req = {
        let agent = ctx.state().agent.lock_safe();
        let cfg = agent.ai_config.source_control.clone().ok_or_else(|| {
            ApiError::from(
                "No commit-message model is configured. Pick one in Settings → Agent → Source Control.",
            )
        })?;
        let entry = agent.ai_config.find_by_key(&cfg.provider_key).ok_or_else(|| {
            ApiError::from(format!(
                "The provider \"{}\" is no longer connected. Re-pick a model in Settings → Agent → Source Control.",
                cfg.provider_key
            ))
        })?;
        CommitMessageRequest {
            provider_key: entry.provider_key(),
            model: cfg.model.clone(),
            api_key: entry.api_key.clone(),
            base_url: entry.base_url.clone(),
            capabilities: agent.ai_config.capabilities_for(&cfg.model),
            allowed_providers: agent.ai_config.allowed_providers_for(&cfg.model),
        }
    };

    let root = project_root(ctx, &a.project_id)?;
    let diff = tokio::task::spawn_blocking(move || {
        let repo = rustic_git::GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
        repo.diff_for_commit_message().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ApiError::from(e.to_string()))?
    .map_err(ApiError::from)?;

    let message = rustic_agent::commit_message::generate_commit_message(req, diff)
        .await
        .map_err(ApiError::from)?;
    ok(json!({ "message": message }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscribeArg {
    audio_base64: String,
    mime: Option<String>,
}

/// Mic speech-to-text. Provider logic lives in
/// `rustic_app::host_ops::agent_audio` (shared with the desktop host).
async fn transcribe_audio(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    use rustic_app::context::EventEmitter;

    let a: TranscribeArg = parse(args)?;

    let (model, api_key, provider_type, base_url) = {
        let agent = ctx.state().agent.lock_safe();
        let cfg = agent
            .ai_config
            .audio_input
            .clone()
            .ok_or_else(|| ApiError::from("No audio input model is configured."))?;
        let entry = agent
            .ai_config
            .find_by_key(&cfg.provider_key)
            .ok_or_else(|| {
                ApiError::from(format!(
                    "The audio provider \"{}\" is no longer connected.",
                    cfg.provider_key
                ))
            })?;
        (
            cfg.model,
            entry.api_key.clone(),
            entry.provider_type.clone(),
            entry.base_url.clone(),
        )
    };

    let on_delta = |text: &str| ctx.emit_json("audio-transcript-delta", json!({ "text": text }));
    let full = rustic_app::host_ops::agent_audio::transcribe(
        provider_type,
        base_url,
        &api_key,
        &model,
        &a.audio_base64,
        a.mime,
        &on_delta,
    )
    .await
    .map_err(ApiError::from)?;

    ctx.emit_json("audio-transcript-done", json!({ "text": full.clone() }));
    ok(json!({ "text": full }))
}

// ════════════════════════════════════════════════════════════════════════
// project defaults  (stored in projects.settings_json)
// ════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectDefaults {
    model: Option<String>,
    provider_type: Option<String>,
    permission_level: Option<String>,
    thinking_effort: Option<String>,
}

fn get_project_defaults(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let db = ctx.state().db.lock_safe();
    if let Ok(Some(project)) = db.get_project(&a.project_id) {
        if let Some(json) = &project.settings_json {
            if let Ok(defaults) = serde_json::from_str::<ProjectDefaults>(json) {
                return ok(defaults);
            }
        }
    }
    ok(ProjectDefaults::default())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveProjectDefaultsArg {
    project_id: String,
    defaults: ProjectDefaults,
}

fn save_project_defaults(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SaveProjectDefaultsArg = parse(args)?;
    let json = serde_json::to_string(&a.defaults).map_err(|e| e.to_string())?;
    let db = ctx.state().db.lock_safe();
    db.update_project_settings(&a.project_id, Some(&json))
        .map_err(|e| e.to_string())?;
    ok(())
}

// ── per-project MCP (mirrors src-tauri/src/commands/agent/mcp.rs) ──────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpProjectRow {
    project_id: String,
    project_name: String,
    root_path: String,
    view: Option<McpProjectServerView>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpProjectAddOutcome {
    project_id: String,
    project_name: String,
    result: Option<McpProjectSaveResult>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpAddResult {
    pool: McpSaveResult,
    projects: Vec<McpProjectAddOutcome>,
}

/// (id, name, root) for every project open in the workspace.
fn workspace_projects(ctx: &ServerContext) -> Vec<(String, String, PathBuf)> {
    ctx.state()
        .workspace
        .lock_safe()
        .list_projects()
        .into_iter()
        .map(|p| (p.id.to_string(), p.name.clone(), p.root_path.clone()))
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMcpPoolServerArg {
    name: String,
    entry: Value,
    #[serde(default)]
    project_ids: Vec<String>,
}

async fn add_mcp_pool_server(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: AddMcpPoolServerArg = parse(args)?;
    let user_path = resolve_scope_path(ctx, McpScope::User, None)?;
    let projects = workspace_projects(ctx);
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Result<McpAddResult, String> {
        let mut mcp = mcp_arc.lock_safe();
        mcp.set_user_path(user_path.clone());
        let _ = mcp.load_scope(McpScope::User, &user_path);
        mcp.add_pool_server(&a.name, &a.entry)
            .map_err(|e| e.to_string())?;
        let pool = match mcp.test_server(&format!("user-{}", a.name)) {
            Ok(tools) => McpSaveResult {
                name: a.name.clone(),
                connected: true,
                tool_count: tools.len(),
                error: None,
            },
            Err(e) => McpSaveResult {
                name: a.name.clone(),
                connected: false,
                tool_count: 0,
                error: Some(e.to_string()),
            },
        };
        let mut outcomes = Vec::new();
        for (pid, pname, root) in projects {
            let selected = a.project_ids.contains(&pid);
            let r = mcp
                .save_project_server(&root, &a.name, None, selected, false)
                .map_err(|e| e.to_string());
            outcomes.push(McpProjectAddOutcome {
                project_id: pid,
                project_name: pname,
                result: r.as_ref().ok().cloned(),
                error: r.err(),
            });
        }
        Ok(McpAddResult {
            pool,
            projects: outcomes,
        })
    })
    .await
    .map_err(|e| format!("add_mcp_pool_server task panicked: {}", e))??;
    ok(res)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerNameArg {
    name: String,
}

async fn get_mcp_server_projects(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: McpServerNameArg = parse(args)?;
    let user_path = resolve_scope_path(ctx, McpScope::User, None)?;
    let projects = workspace_projects(ctx);
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Vec<McpProjectRow> {
        let mut mcp = mcp_arc.lock_safe();
        mcp.set_user_path(user_path.clone());
        let _ = mcp.load_scope(McpScope::User, &user_path);
        projects
            .into_iter()
            .map(|(pid, pname, root)| {
                let r = mcp.project_server_view(&root, &a.name);
                McpProjectRow {
                    project_id: pid,
                    project_name: pname,
                    root_path: root.display().to_string(),
                    view: r.as_ref().ok().cloned(),
                    error: r.err().map(|e| e.to_string()),
                }
            })
            .collect()
    })
    .await
    .map_err(|e| format!("get_mcp_server_projects task panicked: {}", e))?;
    ok(res)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveMcpProjectServerArg {
    name: String,
    project_id: String,
    entry: Option<Value>,
    enabled: bool,
}

async fn save_mcp_project_server(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SaveMcpProjectServerArg = parse(args)?;
    let user_path = resolve_scope_path(ctx, McpScope::User, None)?;
    let root = PathBuf::from(project_root(ctx, &a.project_id)?);
    let mcp_arc = Arc::clone(&ctx.state().agent.lock_safe().mcp_manager);
    let res = tokio::task::spawn_blocking(move || -> Result<McpProjectSaveResult, String> {
        let mut mcp = mcp_arc.lock_safe();
        mcp.set_user_path(user_path.clone());
        let _ = mcp.load_scope(McpScope::User, &user_path);
        mcp.save_project_server(&root, &a.name, a.entry.as_ref(), a.enabled, true)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("save_mcp_project_server task panicked: {}", e))??;
    ok(res)
}
