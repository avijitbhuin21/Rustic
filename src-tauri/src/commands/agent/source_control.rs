//! AI commit-message generation for the Source Control panel.
//!
//! The user picks a provider + model in Settings → Agent → Source Control
//! (stored as `AiConfig::source_control`, same shape as the audio-input
//! pick). The commit form's AI button then calls `generate_commit_message`,
//! which diffs the working tree (staged changes when anything is staged,
//! otherwise all changes including untracked files) and routes it through
//! the shared `rustic_agent::commit_message` generator.

use crate::state::AppState;
use crate::sync_ext::MutexExt;
use rustic_agent::commit_message::CommitMessageRequest;
use std::path::Path;
use tauri::State;

/// Configure the model used for AI commit-message generation. Both fields
/// required; the provider must already be connected.
#[tauri::command]
pub fn set_source_control_config(
    state: State<'_, AppState>,
    provider_key: String,
    model: String,
) -> Result<(), String> {
    let provider_key = provider_key.trim().to_string();
    let model = model.trim().to_string();
    if provider_key.is_empty() || model.is_empty() {
        return Err("provider_key and model are required".to_string());
    }
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    if agent.ai_config.find_by_key(&provider_key).is_none() {
        return Err(format!(
            "No configured provider matches key \"{}\". Pick a model from a \
             provider that's already connected.",
            provider_key
        ));
    }
    agent.ai_config.source_control = Some(rustic_agent::SourceControlConfig {
        provider_key,
        model,
    });
    super::persist_ai_config(&agent.ai_config, &state)?;
    Ok(())
}

/// Remove the configured commit-message model.
#[tauri::command]
pub fn clear_source_control_config(state: State<'_, AppState>) -> Result<(), String> {
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    agent.ai_config.source_control = None;
    super::persist_ai_config(&agent.ai_config, &state)?;
    Ok(())
}

/// Generate a conventional-commit message from the project's current diff
/// using the model configured in Settings → Agent → Source Control.
#[tauri::command]
pub async fn generate_commit_message(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<String, String> {
    // Resolve the configured model + provider credentials, then drop the lock
    // before any await.
    let req = {
        let agent = state.agent.lock().map_err(|e| e.to_string())?;
        let cfg = agent.ai_config.source_control.clone().ok_or_else(|| {
            "No commit-message model is configured. Pick one in Settings → Agent → Source Control."
                .to_string()
        })?;
        let entry = agent.ai_config.find_by_key(&cfg.provider_key).ok_or_else(|| {
            format!(
                "The provider \"{}\" is no longer connected. Re-pick a model in Settings → Agent → Source Control.",
                cfg.provider_key
            )
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

    // Collect the diff on a blocking thread — git CLI work can be slow on
    // large change sets.
    let root = {
        let workspace = state.workspace.lock_safe();
        let project = workspace
            .list_projects()
            .into_iter()
            .find(|p| p.id.to_string() == project_id)
            .ok_or_else(|| format!("Project not found: {}", project_id))?;
        project.root_path.to_string_lossy().to_string()
    };
    let diff = tokio::task::spawn_blocking(move || {
        let repo = rustic_git::GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
        repo.diff_for_commit_message().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    rustic_agent::commit_message::generate_commit_message(req, diff).await
}
