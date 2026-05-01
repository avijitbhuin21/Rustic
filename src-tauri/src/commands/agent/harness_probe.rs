//! Tauri commands for probing harness binaries (Claude Code, Codex).
//!
//! The Subscriptions card in AI settings calls these before letting the user
//! click "Enable" so the failure modes are surfaced clearly:
//!
//! * `NotInstalled`     — the binary couldn't be found / spawned.
//! * `NotAuthenticated` — found and runs `--version`, but no credentials file.
//! * `Authenticated`    — looks healthy.
//! * `ProbeFailed`      — spawned but `--version` errored or hung.
//!
//! Without this, clicking Enable just registered the provider blindly and the
//! user only discovered the failure when they tried to send a message — bad
//! UX, since the actual install/signin step is documented elsewhere and
//! Rustic should help them get there.

use rustic_agent::{probe_claude_code, probe_codex, HarnessAuthStatus};
use std::path::PathBuf;

/// Probe the named harness CLI's auth status. `kind` is the same string the
/// frontend uses as the storage key ("ClaudeCode" / "Codex"); `binary_path`
/// is an optional absolute path override for users whose CLI isn't on PATH.
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
