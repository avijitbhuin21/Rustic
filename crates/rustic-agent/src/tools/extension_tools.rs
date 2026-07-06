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
            name: "install_extension".to_string(),
            description: "Install a skill or workflow so it is available immediately and in \
                 future tasks. Two sources: `content` (you author the markdown yourself — \
                 preferred, auto-approved at project scope) or `url` (pull from the web — \
                 ALWAYS requires explicit user consent). Global scope also requires consent. \
                 The markdown must start with `---` frontmatter containing `name:` (matching \
                 the `name` param) and `description:`. After install, load it with \
                 read_skill / read_workflow."
                .to_string(),
            parameters: json!({
                "type": "object",
                "required": ["kind", "name"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["skill", "workflow"],
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
                "required": ["name", "transport"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Kebab-case server name, unique within the scope."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "user"],
                        "description": "project = <project>/.mcp.json (committed, default); user = global mcp.json shared across projects."
                    },
                    "transport": {
                        "type": "object",
                        "description": "Connection config. stdio: {\"type\":\"stdio\",\"command\":\"npx\",\"args\":[...],\"env\":{...}}. Remote: {\"type\":\"http\",\"url\":\"https://...\",\"headers\":{...}} (\"sse\" is accepted as an alias of \"http\")."
                    }
                }
            }),
        },
        ToolDef {
            name: "uninstall_extension".to_string(),
            description: "Uninstall a skill, workflow, or MCP server. Never destructive: \
                 skills/workflows are moved to ~/.rustic/trash/ (restore by moving back) and \
                 MCP server configs are backed up there before removal."
                .to_string(),
            parameters: json!({
                "type": "object",
                "required": ["kind", "name"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["skill", "workflow", "mcp_server"],
                        "description": "What to uninstall."
                    },
                    "name": {
                        "type": "string",
                        "description": "The extension's name as shown in its listing."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global", "user"],
                        "description": "Disambiguates when the same name exists in two scopes. skills/workflows: project|global; MCP: project|user."
                    }
                }
            }),
        },
    ]
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
    if kind != "skill" && kind != "workflow" {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: kind must be \"skill\" or \"workflow\"",
            true,
        ));
    }
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
            _ => context.project_root.join(".rustic/workflows"),
        }
    } else {
        let dir = match kind.as_str() {
            "skill" => crate::skills::global_skills_dir(),
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
        if dir.exists() {
            return Ok(ToolOutput::text(
                format!(
                    "ALREADY_EXISTS: a skill named `{}` already exists at {}. Uninstall it \
                     first (uninstall_extension) or pick a different name.",
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
        if file.exists() {
            return Ok(ToolOutput::text(
                format!(
                    "ALREADY_EXISTS: a workflow named `{}` already exists at {}. Uninstall \
                     it first (uninstall_extension) or pick a different name.",
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
        let sidecar = workflow_provenance_path(&file);
        if let Ok(text) = serde_json::to_string_pretty(&prov) {
            let _ = std::fs::write(sidecar, text);
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

    let available: Vec<String> = if kind == "skill" {
        crate::skills::discover_skills(&context.project_root)
            .into_iter()
            .map(|s| s.name)
            .collect()
    } else {
        crate::workflows::discover_workflows(&context.project_root)
            .into_iter()
            .map(|w| w.name)
            .collect()
    };

    let body = json!({
        "installed": true,
        "kind": kind,
        "name": name,
        "scope": scope,
        "source": source,
        "sha256": sha256,
        "path": installed_path.display().to_string(),
        "note": format!(
            "Available immediately via {}(\"{}\"). Tell the user what you installed and why.",
            if kind == "skill" { "read_skill" } else { "read_workflow" },
            name
        ),
        "all_available": available,
    });
    Ok(ToolOutput::text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
        false,
    ))
}

async fn add_mcp_server(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let Some(name) = str_param(&params, "name") else {
        return Ok(ToolOutput::text("INVALID_PARAMS: name is required", true));
    };
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
    let Some(transport_val) = params.get("transport").cloned() else {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: transport is required",
            true,
        ));
    };
    let transport: McpTransport = match serde_json::from_value(transport_val.clone()) {
        Ok(t) => t,
        Err(e) => {
            return Ok(ToolOutput::text(
                format!(
                    "INVALID_TRANSPORT: {}. Expected {{\"type\":\"stdio\",\"command\":...}} \
                     or {{\"type\":\"http\",\"url\":...}}.",
                    e
                ),
                true,
            ));
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

    // Ensure the project-scope path is wired even if this project had no
    // .mcp.json when the manager was bootstrapped.
    let project_mcp_path = context.project_root.join(".mcp.json");
    let mgr_clone = std::sync::Arc::clone(mgr);
    let name_for_block = name.clone();
    let add_result = tokio::task::spawn_blocking(move || {
        let mut m = mgr_clone.lock().unwrap();
        if scope == McpScope::Project && m.path_for(McpScope::Project).is_none() {
            m.set_project_path(project_mcp_path);
        }
        m.add_server(scope, &name_for_block, transport)
    })
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

    if !matches!(kind.as_str(), "skill" | "workflow" | "mcp_server") {
        return Ok(ToolOutput::text(
            "INVALID_PARAMS: kind must be \"skill\", \"workflow\", or \"mcp_server\"",
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
