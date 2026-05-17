//! Live model-list commands for harness providers.
//! Codex: spawns an ephemeral app-server and runs the model/list handshake.
//! Claude Code: no list-models API exists; hardcoded tier aliases instead.

use std::path::PathBuf;
use std::time::Duration;

const CODEX_LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// Tier aliases the CLI accepts directly; update when Anthropic adds a new tier.
const CLAUDE_CODE_MODELS: &[&str] = &["opus", "sonnet", "haiku"];

#[tauri::command]
pub fn list_claude_code_models() -> Vec<String> {
    CLAUDE_CODE_MODELS.iter().map(|s| (*s).to_string()).collect()
}

#[tauri::command]
pub async fn list_codex_models(binary_path: Option<String>) -> Result<Vec<String>, String> {
    let path: Option<PathBuf> = binary_path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    rustic_agent::harness::codex::list_codex_models(path, CODEX_LIST_TIMEOUT)
        .await
        .map_err(|e| format!("{e:#}"))
}
