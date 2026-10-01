//! Agent self-extension tools: `install_extension`, `add_mcp_server`,
//! `uninstall_extension`.
//!
//! Consent matrix (see `crate::extensions` for the full safety model):
//! - inline + project-scope skill/workflow → no prompt (AutoEdit/FullAuto),
//!   prompt as a write in ManualEdit.
//! - URL source or global scope → prompt always, in every mode.
//! - MCP servers → prompt always, in every mode.
//! - Uninstalls → prompt in ManualEdit only; always reversible via trash.
//! Sub-agents are rejected outright and told to escalate to the orchestrator.

use crate::extensions::{
    audit, audit_entry, fetch_text, move_to_trash, preview_capped, read_provenance, validate_name,
    workflow_provenance_path, write_provenance, Provenance,
};
use crate::mcp::config::{McpScope, McpTransport};
use crate::mcp::sha256_hex;
use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};

/// Route an extension-tool call to its handler, enforcing the shared gates.
pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    // Read-only: allowed in every mode and for sub-agents.
    if name == "list_extensions" {
        return list_extensions(context).await;
    }
    if context.agent_depth >= 1 {
        return Ok(ToolOutput::text(
            "SUBAGENT_FORBIDDEN: sub-agents cannot install or uninstall extensions. \
             If your task genuinely needs a new skill, workflow, or MCP server, use \
             `escalate_question` to ask the orchestrator to install it.",
            true,
        ));
    }
    if context.is_plan_mode {
        return Ok(ToolOutput::text(
            "PLAN_MODE: extension changes are write operations and are disabled in plan mode.",
            true,
        ));
    }
    if matches!(context.permissions(), PermissionLevel::Chat) {
        return Ok(ToolOutput::text(
            "PERMISSION_DENIED: Chat mode is read-only; extension changes are not allowed.",
            true,
        ));
    }
    match name {
        "install_extension" => install_extension(params, context).await,
        "add_mcp_server" => add_mcp_server(params, context).await,
        "uninstall_extension" => uninstall_extension(params, context).await,
        _ => Ok(ToolOutput::text(
            format!("Unknown extension tool: {}", name),
            true,
        )),
    }
}

/// Tool definitions exposed to the AI provider (all deferred behind tool_search).
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_extensions".to_string(),
            description: "List what is installed before changing it: skills, workflows and rules \
                 (with project/global scope; rules also show whether they are active here), the \
                 global MCP server pool with connection status, and the MCP servers active in this \
                 project. Read-only."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "install_extension".to_string(),
            description: "Install (or, with overwrite=true, update in place) a skill, workflow or \
                 rule so it is available immediately and in future tasks. Two sources: `content` \
                 (you author the markdown yourself — preferred, auto-approved at project scope) or \
                 `url` (pull from the web — ALWAYS requires explicit user consent). Global scope \
                 also requires consent. The markdown must start with `---` frontmatter containing \
                 `name:` (matching the `name` param) and `description:`. Skills/workflows load via \
                 read_skill / read_workflow; rules are injected into the system prompt (project \
                 rules always, global rules where activated)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "required": ["kind", "name"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["skill", "workflow", "rule"],
                        "description": "What to install."
                    },
                    "name": {
                        "type": "string",
                        "description": "Kebab-case identifier (lowercase letters, digits, '-', '_'). Must match the frontmatter `name:`."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global"],
                        "description": "project = <project>/.rustic/... (default); global = ~/.rustic/... for all projects (requires user consent)."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full markdown including frontmatter. Mutually exclusive with `url`."
                    },
                    "url": {
                        "type": "string",
                        "description": "http(s) URL of a markdown file to install. Requires user consent; state where you found it. Mutually exclusive with `content`."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "Update an existing extension with the same name (old copy moved to ~/.rustic/trash/). Default false."
                    },
                    "activate": {
                        "type": "string",
                        "enum": ["project", "global", "none"],
                        "description": "Global rules only: where the rule is active. project = this project (default), global = every project, none = installed but off."
                    }
                }
            }),
        },
        ToolDef {
            name: "add_mcp_server".to_string(),
            description: "Register a new MCP server. ALWAYS requires explicit user consent \
                 (stdio servers execute a local command; remote servers are a live network \
                 channel). On approval the server is saved to the scope's config file and \
                 connected immediately — its tools land in the deferred tools table, so load \
                 their schemas with tool_search before calling them."
                .to_string(),
            parameters: json!({
                "type": "object",
                "required": ["transport"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Server name, unique within the scope. Optional when transport is a {\"mcpServers\":{...}} document with one server (the key is used); picks one server when it lists several."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "user"],
                        "description": "project = this project only: written to <project>/.mcp.json, .gemini/settings.json and .codex/config.toml (git-ignored) so Claude Code / Gemini CLI / Codex see it too (default); user = global pool shared by every project."
                    },
                    "transport": {
                        "type": "object",
                        "description": "Standard MCP JSON, exactly as users paste it for Claude Code / Cursor: either the full document {\"mcpServers\":{\"<name>\":{\"command\":\"uv\",\"args\":[...],\"env\":{...}}}} or a single entry. stdio entry: {\"command\":...,\"args\":[...],\"env\":{...}}; remote entry: {\"url\":\"https://...\",\"headers\":{...}}. \"type\" is optional (inferred from command/url)."
                    }
                }
            }),
        },
        ToolDef {
            name: "uninstall_extension".to_string(),
            description: "Uninstall a skill, workflow, rule, or MCP server. Never destructive: \
                 skills/workflows/rules are moved to ~/.rustic/trash/ (restore by moving back) and \
                 MCP server configs are backed up there before removal."
                .to_string(),
            parameters: json!({
                "type": "object",
                "required": ["kind", "name"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["skill", "workflow", "rule", "mcp_server"],
                        "description": "What to uninstall."
                    },
                    "name": {
                        "type": "string",
                        "description": "The extension's name as shown in its listing."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global", "user"],
                        "description": "Disambiguates when the same name exists in two scopes. skills/workflows/rules: project|global; MCP: project|user."
                    }
                }
            }),
        },
    ]
}

/// Inventory of skills, workflows, rules and MCP servers visible to this project.
async fn list_extensions(context: &ToolContext) -> Result<ToolOutput> {
    let root = context.project_root.clone();
    let skills: Vec<Value> = crate::skills::discover_skills(&root)
        .into_iter()
        .map(|s| json!({ "name": s.name, "scope": format!("{:?}", s.scope).to_lowercase() }))
        .collect();
    let workflows: Vec<Value> = crate::workflows::discover_workflows(&root)
        .into_iter()
        .map(|w| {
            let scope = if w.path.starts_with(&root) {
                "project"
            } else {
                "global"
            };
            json!({ "name": w.name, "scope": scope })
        })
        .collect();
    let mut rules: Vec<Value> = crate::rules::discover_project_rules(&root)
        .into_iter()
        .map(|r| json!({ "name": r.name, "scope": "project", "active": true }))
        .collect();
    for r in crate::rules::discover_global_rules() {
        let state = crate::rules::rule_state(&r.name, &root);
        rules.push(json!({
            "name": r.name,
            "scope": "global",
            "active": state != crate::rules::RuleState::Inactive,
            "activation": format!("{:?}", state).to_lowercase(),
        }));
    }
    let mcp = match context.mcp_manager.as_ref() {
        Some(mgr) => {
            let mgr = std::sync::Arc::clone(mgr);
            let root2 = root.clone();
            tokio::task::spawn_blocking(move || {
                let m = mgr.lock().unwrap();
                let pool: Vec<Value> = m
                    .list_pool_servers_with_status()
                    .into_iter()
                    .map(|s| json!({ "name": s.config.name, "status": s.status }))
                    .collect();
                let active = m.effective_ids_for_project(&root2);
                let here: Vec<Value> = m
                    .list_servers()
                    .into_iter()
                    .filter(|c| active.contains(&c.id))
                    .map(|c| {
                        let source = if c.scope == McpScope::User {
                            "pool"
                        } else {
                            "project override"
                        };
                        json!({ "name": c.name, "source": source })
                    })
                    .collect();
                json!({ "pool": pool, "active_in_this_project": here })
            })
            .await
            .unwrap_or(Value::Null)
        }
        None => Value::Null,
    };
    let body =
        json!({ "skills": skills, "workflows": workflows, "rules": rules, "mcp_servers": mcp });
    Ok(ToolOutput::text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
        false,
    ))
}

fn str_param(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Ask the user for approval through the permission broker; returns an error
/// output when denied.
async fn request_consent(
    context: &ToolContext,
    action: &str,
    kind: &str,
    name: &str,
    preview: String,
) -> Option<ToolOutput> {
    let approved = context
        .permission_broker
        .request(
            &context.event_tx,
            &context.task_id,
            PermissionOp::ExtensionChange {
                action: action.to_string(),
                kind: kind.to_string(),
                name: name.to_string(),
                preview,
            },
        )
        .await;
    if approved {
        None
    } else {
        Some(ToolOutput::text(
            format!(
                "PERMISSION_DENIED: the user declined the {} of {} `{}`. Do not retry \
                 without discussing it with the user first.",
                action, kind, name
            ),
            true,
        ))
    }
}

async fn install_extension(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let kind = str_param(&params, "kind").unwrap_or_default();
    if kind != "skill" && kind != "workflow" && kind != "rule" {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: kind must be \"skill\", \"workflow\" or \"rule\"",
            true,
        ));
    }
    let overwrite = params
        .get("overwrite")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let Some(name) = str_param(&params, "name") else {
        return Ok(ToolOutput::text("INVALID_PARAMS: name is required", true));
    };
    if let Err(e) = validate_name(&name) {
        return Ok(ToolOutput::text(format!("INVALID_PARAMS: {}", e), true));
    }
    let scope = str_param(&params, "scope").unwrap_or_else(|| "project".to_string());
    if scope != "project" && scope != "global" {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: scope must be \"project\" or \"global\"",
            true,
        ));
    }
    let inline = str_param(&params, "content");
    let url = str_param(&params, "url");
    let (content, source) = match (inline, url) {
        (Some(c), None) => (c, "inline".to_string()),
        (None, Some(u)) => match fetch_text(&u).await {
            Ok(c) => (c, u),
            Err(e) => {
                return Ok(ToolOutput::text(format!("FETCH_FAILED: {}", e), true));
            }
        },
        (Some(_), Some(_)) => {
            return Ok(ToolOutput::text(
                "INVALID_PARAMS: provide either `content` or `url`, not both",
                true,
            ));
        }
        (None, None) => {
            return Ok(ToolOutput::text(
                "INVALID_PARAMS: one of `content` (self-authored) or `url` (external) is required",
                true,
            ));
        }
    };

    // Frontmatter validation — the installed artifact must be discoverable
    // and its advertised name must match what the user consented to.
    let fm_name = match kind.as_str() {
        "skill" => crate::skills::parse_skill_frontmatter(&content).map(|(n, _, _)| n),
        "rule" => crate::rules::parse_rule_frontmatter(&content).map(|(n, _)| n),
        _ => crate::workflows::parse_workflow_frontmatter(&content).map(|(n, _)| n),
    };
    match fm_name {
        None => {
            return Ok(ToolOutput::text(
                "INVALID_CONTENT: the markdown must start with `---` frontmatter containing \
                 at least `name:` and `description:` lines, followed by a closing `---`.",
                true,
            ));
        }
        Some(fm) if fm != name => {
            return Ok(ToolOutput::text(
                format!(
                    "NAME_MISMATCH: frontmatter declares `name: {}` but the install was \
                     requested as `{}`. Make them identical.",
                    fm, name
                ),
                true,
            ));
        }
        Some(_) => {}
    }

    let sha256 = sha256_hex(content.as_bytes());
    let external = source != "inline";
    let needs_consent = external
        || scope == "global"
        || matches!(context.permissions(), PermissionLevel::ManualEdit);
    if needs_consent {
        let preview = format!(
            "Kind: {}\nScope: {}\nSource: {}\nSHA-256: {}\n\n--- CONTENT ---\n{}",
            kind,
            scope,
            source,
            sha256,
            preview_capped(&content)
        );
        if let Some(denied) = request_consent(context, "install", &kind, &name, preview).await {
            return Ok(denied);
        }
    }

    // Resolve the destination and refuse to overwrite anything that exists.
    let base = if scope == "project" {
        match kind.as_str() {
            "skill" => context.project_root.join(".rustic/skills"),
            "rule" => crate::rules::project_rules_dir(&context.project_root),
            _ => context.project_root.join(".rustic/workflows"),
        }
    } else {
        let dir = match kind.as_str() {
            "skill" => crate::skills::global_skills_dir(),
            "rule" => crate::rules::global_rules_dir(),
            _ => crate::workflows::global_workflows_dir(),
        };
        match dir {
            Some(d) => d,
            None => {
                return Ok(ToolOutput::text(
                    "INSTALL_FAILED: cannot resolve the home directory for global scope",
                    true,
                ));
            }
        }
    };

    let prov = Provenance {
        origin: "agent".to_string(),
        source: source.clone(),
        sha256: sha256.clone(),
        installed_at: chrono::Utc::now().to_rfc3339(),
        task_id: context.task_id.clone(),
    };

    let installed_path = if kind == "skill" {
        let dir = base.join(&name);
        if dir.exists() && overwrite {
            if let Err(e) = move_to_trash(&dir) {
                return Ok(ToolOutput::text(
                    format!("INSTALL_FAILED: could not back up existing skill: {}", e),
                    true,
                ));
            }
        }
        if dir.exists() {
            return Ok(ToolOutput::text(
                format!(
                    "ALREADY_EXISTS: a skill named `{}` already exists at {}. Pass \
                     overwrite=true to update it in place (the old copy goes to ~/.rustic/trash/), \
                     or pick a different name.",
                    name,
                    dir.display()
                ),
                true,
            ));
        }
        if let Err(e) = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(dir.join("SKILL.md"), &content))
        {
            return Ok(ToolOutput::text(format!("INSTALL_FAILED: {}", e), true));
        }
        if let Err(e) = write_provenance(&dir, &prov) {
            tracing::warn!("failed to write skill provenance: {}", e);
        }
        dir.join("SKILL.md")
    } else {
        let file = base.join(format!("{}.md", name));
        if file.exists() && overwrite {
            if let Err(e) = move_to_trash(&file) {
                return Ok(ToolOutput::text(
                    format!("INSTALL_FAILED: could not back up existing {}: {}", kind, e),
                    true,
                ));
            }
        }
        if file.exists() {
            return Ok(ToolOutput::text(
                format!(
                    "ALREADY_EXISTS: a {} named `{}` already exists at {}. Pass \
                     overwrite=true to update it in place (the old copy goes to ~/.rustic/trash/), \
                     or pick a different name.",
                    kind,
                    name,
                    file.display()
                ),
                true,
            ));
        }
        if let Err(e) = std::fs::create_dir_all(&base).and_then(|_| std::fs::write(&file, &content))
        {
            return Ok(ToolOutput::text(format!("INSTALL_FAILED: {}", e), true));
        }
        if kind == "workflow" {
            let sidecar = workflow_provenance_path(&file);
            if let Ok(text) = serde_json::to_string_pretty(&prov) {
                let _ = std::fs::write(sidecar, text);
            }
        }
        // Global rules only apply where activated. Default: this project.
        if kind == "rule" && scope == "global" {
            let activate = str_param(&params, "activate").unwrap_or_else(|| "project".to_string());
            let state = match activate.as_str() {
                "global" => crate::rules::RuleState::Global,
                "none" => crate::rules::RuleState::Inactive,
                _ => crate::rules::RuleState::Project,
            };
            if let Err(e) = crate::rules::set_rule_state(&name, state, &context.project_root) {
                tracing::warn!("failed to activate rule {}: {}", name, e);
            }
        }
        file
    };

    audit(&audit_entry(
        "install",
        &kind,
        &name,
        &scope,
        &source,
        Some(sha256.clone()),
        &context.task_id,
        None,
    ));

    let available: Vec<String> = match kind.as_str() {
        "skill" => crate::skills::discover_skills(&context.project_root)
            .into_iter()
            .map(|s| s.name)
            .collect(),
        "rule" => {
            let mut all = crate::rules::discover_project_rules(&context.project_root);
            all.extend(crate::rules::discover_global_rules());
            all.into_iter().map(|r| r.name).collect()
        }
        _ => crate::workflows::discover_workflows(&context.project_root)
            .into_iter()
            .map(|w| w.name)
            .collect(),
    };

    let note = match kind.as_str() {
        "skill" => format!("Available immediately via read_skill(\"{}\"). Tell the user what you installed and why.", name),
        "workflow" => format!("Available immediately via read_workflow(\"{}\"). Tell the user what you installed and why.", name),
        _ => "Rules are injected into the system prompt from the next turn (project rules always; global rules where activated). Tell the user what you added and why.".to_string(),
    };

    let body = json!({
        "installed": true,
        "kind": kind,
        "name": name,
        "scope": scope,
        "source": source,
        "sha256": sha256,
        "path": installed_path.display().to_string(),
        "overwritten": overwrite,
        "note": note,
        "all_available": available,
    });
    Ok(ToolOutput::text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
        false,
    ))
}

async fn add_mcp_server(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let name_param = str_param(&params, "name");
    let Some(raw_transport) = params.get("transport") else {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: transport is required",
            true,
        ));
    };
    let pairs = match crate::mcp::normalize_server_input(raw_transport, name_param.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            return Ok(ToolOutput::text(format!("INVALID_TRANSPORT: {}", e), true));
        }
    };
    if pairs.len() > 1 {
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        return Ok(ToolOutput::text(
            format!(
                "INVALID_PARAMS: the config lists {} servers ({}); call add_mcp_server once per server, passing `name` to pick one.",
                names.len(),
                names.join(", ")
            ),
            true,
        ));
    }
    let (name, transport_val) = pairs.into_iter().next().expect("non-empty pairs");
    if let Err(e) = validate_name(&name) {
        return Ok(ToolOutput::text(format!("INVALID_PARAMS: {}", e), true));
    }
    let scope_str = str_param(&params, "scope").unwrap_or_else(|| "project".to_string());
    let scope = match scope_str.as_str() {
        "project" => McpScope::Project,
        "user" => McpScope::User,
        _ => {
            return Ok(ToolOutput::text(
                "INVALID_PARAMS: scope must be \"project\" or \"user\"",
                true,
            ));
        }
    };
    let transport: McpTransport = match crate::mcp::entry_transport(&name, &transport_val) {
        Ok(t) => t,
        Err(e) => {
            return Ok(ToolOutput::text(format!("INVALID_TRANSPORT: {}", e), true));
        }
    };
    let Some(mgr) = context.mcp_manager.as_ref() else {
        return Ok(ToolOutput::text(
            "MCP_UNAVAILABLE: this host did not wire an MCP manager; cannot add servers here.",
            true,
        ));
    };

    // MCP servers ALWAYS require consent — no tier, no session allowlist.
    let risk_line = match &transport {
        McpTransport::Stdio { command, args, .. } => format!(
            "This will EXECUTE the local command `{} {}` and keep it running as a tool server.",
            command,
            args.join(" ")
        ),
        McpTransport::Sse { url, .. } => format!(
            "This will open a persistent network connection to `{}` and send tool data to it.",
            url
        ),
    };
    let preview = format!(
        "Scope: {}\n{}\n\n--- CONFIG ---\n{}",
        scope_str,
        risk_line,
        serde_json::to_string_pretty(&transport_val).unwrap_or_default()
    );
    if let Some(denied) = request_consent(context, "install", "mcp_server", &name, preview).await {
        return Ok(denied);
    }

    // project = write into this project's .mcp.json / .gemini / .codex
    // (git-ignored) and connect with that config; user = add to the global
    // pool so every project gets it (issue #11, per-project MCP model).
    let root = context.project_root.clone();
    let entry = transport_val.clone();
    let mgr_clone = std::sync::Arc::clone(mgr);
    let name_for_block = name.clone();
    let add_result = tokio::task::spawn_blocking(
        move || -> anyhow::Result<(String, std::result::Result<Vec<crate::provider::ToolDef>, String>)> {
            let mut m = mgr_clone.lock().unwrap();
            match scope {
                McpScope::User => {
                    m.add_pool_server(&name_for_block, &entry)?;
                    let id = format!("user-{}", name_for_block);
                    let connect = m.test_server(&id).map_err(|e| e.to_string());
                    Ok((id, connect))
                }
                McpScope::Project => {
                    let res = m.save_project_server(&root, &name_for_block, Some(&entry), true, true)?;
                    let id = m
                        .effective_config_for_project(&root, &name_for_block)
                        .map(|c| c.id)
                        .unwrap_or_else(|| name_for_block.clone());
                    let connect = if res.consent_required {
                        Err("the project's .mcp.json has changes not yet approved in Rustic; ask the user to approve it in Settings → MCP".to_string())
                    } else if res.connected {
                        m.server_tools(&id).map_err(|e| e.to_string())
                    } else {
                        Err(res.error.unwrap_or_else(|| "connection failed".to_string()))
                    };
                    Ok((id, connect))
                }
            }
        },
    )
    .await;

    let (id, connect) = match add_result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Ok(ToolOutput::text(format!("ADD_FAILED: {}", e), true)),
        Err(e) => {
            return Ok(ToolOutput::text(
                format!("ADD_FAILED: task panicked: {}", e),
                true,
            ))
        }
    };

    audit(&audit_entry(
        "install",
        "mcp_server",
        &name,
        &scope_str,
        &serde_json::to_string(&transport_val).unwrap_or_default(),
        None,
        &context.task_id,
        connect.as_ref().err().cloned(),
    ));

    match connect {
        Ok(tools) => {
            // Surface the new tools through the deferred table so tool_search
            // can load their schemas this very turn; the full provider tool
            // list picks them up on the next turn's reassembly.
            let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
            if let Ok(mut table) = context.deferred_tools.lock() {
                for t in tools {
                    if !table.iter().any(|d| d.name == t.name) {
                        table.push(t);
                    }
                }
            }
            let body = json!({
                "installed": true,
                "server_id": id,
                "scope": scope_str,
                "connected": true,
                "tool_count": tool_names.len(),
                "tools": tool_names,
                "note": "Server saved and connected. Load any tool's schema with \
                         tool_search (e.g. query \"select:NAME\") before calling it.",
            });
            Ok(ToolOutput::text(
                serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
                false,
            ))
        }
        Err(e) => Ok(ToolOutput::text(
            format!(
                "INSTALLED_BUT_NOT_CONNECTED: server `{}` was saved to {} scope but the \
                 initial connection failed: {}. Fix the config (uninstall + re-add) or ask \
                 the user to check it in Settings.",
                name, scope_str, e
            ),
            true,
        )),
    }
}

async fn uninstall_extension(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let kind = str_param(&params, "kind").unwrap_or_default();
    let Some(name) = str_param(&params, "name") else {
        return Ok(ToolOutput::text("INVALID_PARAMS: name is required", true));
    };
    let scope_filter = str_param(&params, "scope");

    if !matches!(kind.as_str(), "skill" | "workflow" | "rule" | "mcp_server") {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: kind must be \"skill\", \"workflow\", \"rule\", or \"mcp_server\"",
            true,
        ));
    }

    if matches!(context.permissions(), PermissionLevel::ManualEdit) {
        let preview = format!(
            "Uninstall {} `{}`{}. Files are moved to ~/.rustic/trash/ (reversible); MCP \
             configs are backed up there before removal.",
            kind.replace('_', " "),
            name,
            scope_filter
                .as_ref()
                .map(|s| format!(" from {} scope", s))
                .unwrap_or_default()
        );
        if let Some(denied) = request_consent(context, "uninstall", &kind, &name, preview).await {
            return Ok(denied);
        }
    }

    match kind.as_str() {
        "skill" => {
            let skills = crate::skills::discover_skills(&context.project_root);
            let matched: Vec<_> = skills
                .into_iter()
                .filter(|s| s.name == name)
                .filter(|s| match scope_filter.as_deref() {
                    Some("project") => s.scope == crate::skills::SkillScope::Project,
                    Some("global") => s.scope == crate::skills::SkillScope::Global,
                    _ => true,
                })
                .collect();
            let Some(skill) = matched.first() else {
                return Ok(ToolOutput::text(
                    format!(
                        "NOT_FOUND: no skill named `{}` in the requested scope",
                        name
                    ),
                    true,
                ));
            };
            let dir = skill
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| skill.path.clone());
            let external = read_provenance(&dir)
                .map(|p| p.is_external())
                .unwrap_or(false);
            match move_to_trash(&dir) {
                Ok(dest) => {
                    audit(&audit_entry(
                        "uninstall",
                        "skill",
                        &name,
                        &format!("{:?}", skill.scope).to_lowercase(),
                        if external { "external" } else { "local" },
                        None,
                        &context.task_id,
                        Some(format!("trashed to {}", dest.display())),
                    ));
                    Ok(ToolOutput::text(
                        format!(
                            "Uninstalled skill `{}`. Backed up to {} — restore by moving \
                             the folder back.",
                            name,
                            dest.display()
                        ),
                        false,
                    ))
                }
                Err(e) => Ok(ToolOutput::text(format!("UNINSTALL_FAILED: {}", e), true)),
            }
        }
        "workflow" => {
            let workflows = crate::workflows::discover_workflows(&context.project_root);
            let in_project = |p: &std::path::Path| p.starts_with(&context.project_root);
            let matched: Vec<_> = workflows
                .into_iter()
                .filter(|w| w.name == name)
                .filter(|w| match scope_filter.as_deref() {
                    Some("project") => in_project(&w.path),
                    Some("global") => !in_project(&w.path),
                    _ => true,
                })
                .collect();
            let Some(wf) = matched.first() else {
                return Ok(ToolOutput::text(
                    format!(
                        "NOT_FOUND: no workflow named `{}` in the requested scope",
                        name
                    ),
                    true,
                ));
            };
            match move_to_trash(&wf.path) {
                Ok(dest) => {
                    let sidecar = workflow_provenance_path(&wf.path);
                    if sidecar.exists() {
                        let _ = move_to_trash(&sidecar);
                    }
                    audit(&audit_entry(
                        "uninstall",
                        "workflow",
                        &name,
                        if in_project(&wf.path) {
                            "project"
                        } else {
                            "global"
                        },
                        "local",
                        None,
                        &context.task_id,
                        Some(format!("trashed to {}", dest.display())),
                    ));
                    Ok(ToolOutput::text(
                        format!(
                            "Uninstalled workflow `{}`. Backed up to {} — restore by moving \
                             the file back.",
                            name,
                            dest.display()
                        ),
                        false,
                    ))
                }
                Err(e) => Ok(ToolOutput::text(format!("UNINSTALL_FAILED: {}", e), true)),
            }
        }
        "rule" => {
            let mut rules = crate::rules::discover_project_rules(&context.project_root);
            rules.extend(crate::rules::discover_global_rules());
            let matched: Vec<_> = rules
                .into_iter()
                .filter(|r| r.name == name)
                .filter(|r| match scope_filter.as_deref() {
                    Some("project") => r.scope == crate::rules::RuleScope::Project,
                    Some("global") => r.scope == crate::rules::RuleScope::Global,
                    _ => true,
                })
                .collect();
            let Some(rule) = matched.first() else {
                return Ok(ToolOutput::text(
                    format!("NOT_FOUND: no rule named `{}` in the requested scope", name),
                    true,
                ));
            };
            match move_to_trash(&rule.path) {
                Ok(dest) => {
                    if rule.scope == crate::rules::RuleScope::Global {
                        let _ = crate::rules::forget_rule(&name);
                    }
                    let scope_str = if rule.scope == crate::rules::RuleScope::Project {
                        "project"
                    } else {
                        "global"
                    };
                    audit(&audit_entry(
                        "uninstall",
                        "rule",
                        &name,
                        scope_str,
                        "local",
                        None,
                        &context.task_id,
                        Some(format!("trashed to {}", dest.display())),
                    ));
                    Ok(ToolOutput::text(
                        format!(
                            "Removed rule `{}`. Backed up to {} — restore by moving the file back.",
                            name,
                            dest.display()
                        ),
                        false,
                    ))
                }
                Err(e) => Ok(ToolOutput::text(format!("UNINSTALL_FAILED: {}", e), true)),
            }
        }
        _ => {
            let Some(mgr) = context.mcp_manager.as_ref() else {
                return Ok(ToolOutput::text(
                    "MCP_UNAVAILABLE: this host did not wire an MCP manager.",
                    true,
                ));
            };
            let mgr_clone = std::sync::Arc::clone(mgr);
            let name_c = name.clone();
            let scope_c = scope_filter.clone();
            // project scope = disable in THIS project only (removed from its
            // .mcp.json / .gemini / .codex, recorded in .rustic/mcp.json); the
            // pool entry and other projects are untouched.
            if scope_c.as_deref() == Some("project") {
                let root = context.project_root.clone();
                let res = tokio::task::spawn_blocking(move || {
                    mgr_clone
                        .lock()
                        .unwrap()
                        .save_project_server(&root, &name_c, None, false, false)
                })
                .await;
                return Ok(match res {
                    Ok(Ok(_)) => {
                        audit(&audit_entry(
                            "uninstall",
                            "mcp_server",
                            &name,
                            "project",
                            "local",
                            None,
                            &context.task_id,
                            Some("disabled for this project".to_string()),
                        ));
                        ToolOutput::text(
                            format!(
                                "Disabled MCP server `{}` for this project (removed from .mcp.json, \
                                 .gemini/settings.json and .codex/config.toml). Re-enable it in \
                                 Settings → MCP → Configure or with add_mcp_server scope=project.",
                                name
                            ),
                            false,
                        )
                    }
                    Ok(Err(e)) => ToolOutput::text(format!("UNINSTALL_FAILED: {}", e), true),
                    Err(e) => {
                        ToolOutput::text(format!("UNINSTALL_FAILED: task panicked: {}", e), true)
                    }
                });
            }
            let mgr_clone = std::sync::Arc::clone(mgr);
            let name_c = name.clone();
            let result =
                tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String, String)> {
                    let mut m = mgr_clone.lock().unwrap();
                    let matched: Vec<_> = m
                        .list_servers()
                        .into_iter()
                        .filter(|c| c.name == name_c)
                        .filter(|c| match scope_c.as_deref() {
                            Some("project") => c.scope == McpScope::Project,
                            Some("user") => c.scope == McpScope::User,
                            _ => true,
                        })
                        .collect();
                    if matched.is_empty() {
                        anyhow::bail!("no MCP server named `{}` in the requested scope", name_c);
                    }
                    if matched.len() > 1 {
                        anyhow::bail!(
                        "`{}` exists in multiple scopes — pass `scope` (\"project\" or \"user\")",
                        name_c
                    );
                    }
                    let cfg = matched.into_iter().next().unwrap();
                    let backup = serde_json::to_string_pretty(&cfg)?;
                    m.remove_server(&cfg.id)?;
                    Ok((cfg.id, format!("{:?}", cfg.scope).to_lowercase(), backup))
                })
                .await;

            match result {
                Ok(Ok((id, scope_label, backup))) => {
                    // Best-effort config backup into trash for manual rollback.
                    let backup_note = crate::extensions::trash_dir()
                        .and_then(|dir| {
                            std::fs::create_dir_all(&dir).ok()?;
                            let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                            let dest = dir.join(format!("{}-mcp-{}.json", stamp, name));
                            std::fs::write(&dest, &backup).ok()?;
                            Some(dest.display().to_string())
                        })
                        .unwrap_or_else(|| "backup write failed".to_string());
                    audit(&audit_entry(
                        "uninstall",
                        "mcp_server",
                        &name,
                        &scope_label,
                        "local",
                        None,
                        &context.task_id,
                        Some(format!("id {}; backup {}", id, backup_note)),
                    ));
                    Ok(ToolOutput::text(
                        format!(
                            "Removed MCP server `{}` ({} scope). Config backed up to {} — \
                             re-add it with add_mcp_server to roll back.",
                            name, scope_label, backup_note
                        ),
                        false,
                    ))
                }
                Ok(Err(e)) => Ok(ToolOutput::text(format!("UNINSTALL_FAILED: {}", e), true)),
                Err(e) => Ok(ToolOutput::text(
                    format!("UNINSTALL_FAILED: task panicked: {}", e),
                    true,
                )),
            }
        }
    }
}

#[cfg(test)]
mod rule_install_tests {
    use super::*;

    const RULE: &str = "---\nname: no-emojis\ndescription: Never use emojis\n---\nDo not use emojis in code or docs.\n";

    /// Fresh temp project root.
    fn root() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rustic-rule-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn installs_project_rule_and_refuses_duplicate_without_overwrite() {
        let dir = root();
        let (ctx, _rx) = ToolContext::new_test(dir.clone());
        let out = execute(
            "install_extension",
            json!({"kind": "rule", "name": "no-emojis", "content": RULE}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!out.is_error, "install failed: {}", out.content);
        let path = crate::rules::project_rules_dir(&dir).join("no-emojis.md");
        assert!(path.is_file());
        assert!(crate::rules::discover_project_rules(&dir)
            .iter()
            .any(|r| r.name == "no-emojis"));

        let dup = execute(
            "install_extension",
            json!({"kind": "rule", "name": "no-emojis", "content": RULE}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(
            dup.is_error && dup.content.contains("ALREADY_EXISTS"),
            "{}",
            dup.content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rule_name_must_match_frontmatter() {
        let dir = root();
        let (ctx, _rx) = ToolContext::new_test(dir.clone());
        let out = execute(
            "install_extension",
            json!({"kind": "rule", "name": "other-name", "content": RULE}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(
            out.is_error && out.content.contains("NAME_MISMATCH"),
            "{}",
            out.content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_extensions_reports_installed_project_rule() {
        let dir = root();
        let (ctx, _rx) = ToolContext::new_test(dir.clone());
        execute(
            "install_extension",
            json!({"kind": "rule", "name": "no-emojis", "content": RULE}),
            &ctx,
        )
        .await
        .unwrap();
        let out = execute("list_extensions", json!({}), &ctx).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("valid JSON");
        let rules = v["rules"].as_array().expect("rules array");
        assert!(rules
            .iter()
            .any(|r| r["name"] == "no-emojis" && r["scope"] == "project" && r["active"] == true));
        assert!(v["skills"].is_array() && v["workflows"].is_array());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
