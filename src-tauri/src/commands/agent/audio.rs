//! Speech-to-text for the prompt-box mic.
//!
//! Transcribes a recorded audio clip (16 kHz mono WAV, base64) using whichever
//! provider the user picked as their "audio input agent". The provider-specific
//! transcription lives in `rustic_app::host_ops::agent_audio` (shared with
//! rustic-server); this adapter reads the config and emits
//! `audio-transcript-delta` / `audio-transcript-done`.

use crate::state::AppState;
use rustic_app::host_ops::agent_audio;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize, Clone)]
struct TranscriptDelta {
    text: String,
}

#[derive(Serialize, Clone)]
pub struct TranscriptResult {
    pub text: String,
}

/// Transcribe a base64-encoded audio clip using the configured audio-input
/// model, streaming partial text to the frontend via `audio-transcript-delta`
/// and returning the full transcript.
#[tauri::command]
pub async fn transcribe_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    audio_base64: String,
    mime: Option<String>,
) -> Result<TranscriptResult, String> {
    // Pull the model + provider credentials from the in-memory ai_config (the
    // keychain-hydrated copy that holds the real API keys).
    let (model, api_key, provider_type, base_url) = {
        let agent = state.agent.lock().map_err(|e| e.to_string())?;
        let cfg = agent
            .ai_config
            .audio_input
            .clone()
            .ok_or_else(|| "No audio input model is configured.".to_string())?;
        let entry = agent
            .ai_config
            .find_by_key(&cfg.provider_key)
            .ok_or_else(|| {
                format!(
                    "The audio provider \"{}\" is no longer connected.",
                    cfg.provider_key
                )
            })?;
        (
            cfg.model,
            entry.api_key.clone(),
            entry.provider_type.clone(),
            entry.base_url.clone(),
        )
    };

    let on_delta = |text: &str| {
        let _ = app.emit(
            "audio-transcript-delta",
            TranscriptDelta {
                text: text.to_string(),
            },
        );
    };
    let full = agent_audio::transcribe(
        provider_type,
        base_url,
        &api_key,
        &model,
        &audio_base64,
        mime,
        &on_delta,
    )
    .await?;

    let _ = app.emit(
        "audio-transcript-done",
        TranscriptResult { text: full.clone() },
    );
    Ok(TranscriptResult { text: full })
}
