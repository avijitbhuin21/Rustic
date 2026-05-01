//! Live model-list commands for the harness providers.
//!
//! For each subscription provider we want the "Models" picker in AI Settings
//! and the chat-view model dropdown to reflect what the user's CLI currently
//! advertises. That way new tiers ship without a Rustic update.
//!
//! * **Codex** — has a `model/list` JSON-RPC method on `codex app-server`.
//!   We spawn an ephemeral process, run the `initialize` → `model/list`
//!   handshake, return the IDs.
//!
//! * **Claude Code** — has *no* equivalent. The `claude --help` output has
//!   no list-models flag and there's no slash command that prints a list.
//!   We hardcode a small alias set instead (`sonnet` / `opus` / `haiku`)
//!   that always points to the latest tier — those aliases haven't moved
//!   in Anthropic's CLI for many releases. The runtime user-override path
//!   stays via the existing `/model` slash command in the chat input.

use std::path::PathBuf;
use std::time::Duration;

/// How long to wait for the `codex app-server` handshake before giving up.
/// Generous enough to cover a cold-start where Codex authenticates against
/// `~/.codex/auth.json` over the network on first call.
const CODEX_LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// Static list of Claude Code aliases the picker offers. The CLI accepts
/// these directly via `--model <alias>` and resolves them to whichever
/// concrete version is current. Update only when Anthropic adds a new tier.
const CLAUDE_CODE_MODELS: &[&str] = &["opus", "sonnet", "haiku"];

/// Returns the Claude Code model alias list. Pure constant — kept as a
/// command (rather than a JS-side constant) so the same source of truth
/// drives both the picker and any future backend code that wants to
/// validate a stored selection.
#[tauri::command]
pub fn list_claude_code_models() -> Vec<String> {
    CLAUDE_CODE_MODELS.iter().map(|s| (*s).to_string()).collect()
}

/// Spawn `codex app-server`, run the model/list handshake, and return the
/// model IDs as strings. Errors are bubbled up so the caller can render
/// "CLI not found / not signed in" instead of an empty silent list.
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
