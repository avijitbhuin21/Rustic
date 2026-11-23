//! Inline editor chat (Ctrl+I in the code editor).
//!
//! The prompt, reply protocol and streaming call live in
//! `rustic_agent::inline_edit` (shared with rustic-server); this adapter
//! resolves the picked provider and forwards stream snapshots as
//! `inline-edit-delta`.

use crate::state::AppState;
use rustic_agent::inline_edit::{
    self, InlineEditInput, InlineEditOutput, InlineEditTarget, DELTA_EVENT,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Run one inline request on the picked model, streaming snapshots to the
/// editor and returning the final edit or answer.
#[tauri::command]
pub async fn inline_edit(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_type: String,
    model: String,
    input: InlineEditInput,
) -> Result<InlineEditOutput, String> {
    // Resolve credentials, then drop the lock before any await.
    let target = {
        let agent = state.agent.lock().map_err(|e| e.to_string())?;
        InlineEditTarget::resolve(&agent.ai_config, &provider_type, &model)?
    };
    inline_edit::run_inline_edit(
        target,
        input,
        Arc::new(move |delta| {
            let _ = app.emit(DELTA_EVENT, delta);
        }),
    )
    .await
}

/// Cancel an in-flight inline request. Returns false when it already ended.
#[tauri::command]
pub async fn cancel_inline_edit(request_id: String) -> Result<bool, String> {
    Ok(inline_edit::cancel(&request_id))
}
