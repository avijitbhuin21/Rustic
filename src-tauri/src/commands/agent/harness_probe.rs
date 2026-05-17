//! Auth-status probes for harness binaries (Claude Code, Codex).
//! Called before enabling a subscription provider so install/signin failures
//! are surfaced clearly rather than discovered on first message-send.

use rustic_agent::{probe_claude_code, probe_codex, HarnessAuthStatus};
use std::path::PathBuf;

#[tauri::command]
pub async fn probe_harness_auth(
    kind: String,
    binary_path: Option<String>,
) -> Result<HarnessAuthStatus, String> {
    let path: Option<PathBuf> = binary_path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    let status = match kind.as_str() {
        "ClaudeCode" => probe_claude_code(path.as_deref()).await,
        "Codex" => probe_codex(path.as_deref()).await,
        other => return Err(format!("Unknown harness kind: {other}")),
    };
    Ok(status)
}
