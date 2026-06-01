//! Speech-to-text for the prompt-box mic.
//!
//! Transcribes a recorded audio clip using whichever provider the user picked
//! as their "audio input agent" (see `AudioInputConfig`). The clip is recorded
//! in the webview (MediaRecorder, re-encoded to 16 kHz mono WAV) and handed
//! over base64-encoded.
//!
//! Each provider family transcribes through a different mechanism — there is no
//! single endpoint that works everywhere — so `transcribe_audio` dispatches on
//! the provider type:
//!
//! * **OpenAI / Compatible** → the OpenAI-style `/audio/transcriptions` (Whisper)
//!   endpoint. `gpt-4o-transcribe` / `gpt-4o-mini-transcribe` stream an SSE feed
//!   of `transcript.text.delta` events; `whisper-1` (and most gateways) return a
//!   single `{ "text": ... }` JSON body.
//! * **OpenRouter** → no transcription endpoint exists, so we send the clip as
//!   an `input_audio` content block to `/chat/completions` (`format: "wav"`)
//!   with a "transcribe this" instruction and read back the assistant text.
//! * **Gemini** → native `:streamGenerateContent` with the audio as `inline_data`
//!   plus a transcription prompt; we read the `candidates[].content.parts[].text`.
//!
//! Whatever the path, partial text is emitted to the frontend via
//! `audio-transcript-delta` as it arrives (so words appear in the composer while
//! the rest is still being transcribed) and a final `audio-transcript-done`
//! carries the full transcript. The frontend code path is identical either way.

use crate::state::AppState;
use base64::Engine;
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

/// Emit a partial-transcript delta to the composer.
fn emit_delta(app: &AppHandle, text: &str) {
    if text.is_empty() {
        return;
    }
    let _ = app.emit("audio-transcript-delta", TranscriptDelta { text: text.to_string() });
}

/// Emit the terminal full-transcript event and return it as the command result.
fn finish(app: &AppHandle, full: String) -> Result<TranscriptResult, String> {
    let _ = app.emit("audio-transcript-done", TranscriptResult { text: full.clone() });
    Ok(TranscriptResult { text: full })
}

/// Normalise an OpenAI-compatible API base to its root (no trailing slash, no
/// `/audio/transcriptions`, `/chat/completions`, or `/completions` suffix) so a
/// base saved as `.../openai`, `.../v1`, or `.../chat/completions` all resolve
/// to the right endpoint.
fn normalize_openai_base(raw: &str) -> String {
    raw.trim()
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/completions")
        .trim_end_matches('/')
        .to_string()
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
    use rustic_agent::ProviderType::*;

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

    if api_key.trim().is_empty() {
        return Err("The audio provider has no API key configured.".to_string());
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_base64.as_bytes())
        .map_err(|e| format!("Invalid audio payload: {e}"))?;
    if bytes.is_empty() {
        return Err("Empty audio clip.".to_string());
    }
    // The frontend re-encodes to WAV before sending; default the MIME to match.
    let mime = mime.unwrap_or_else(|| "audio/wav".to_string());

    match provider_type {
        OpenAi => {
            transcribe_via_openai_endpoint(&app, "https://api.openai.com/v1", &api_key, &model, bytes, &mime).await
        }
        Compatible => {
            let raw = base_url.as_deref().unwrap_or("").trim();
            if raw.is_empty() {
                return Err("This Compatible provider has no base URL configured.".to_string());
            }
            let base = normalize_openai_base(raw);
            transcribe_via_openai_endpoint(&app, &base, &api_key, &model, bytes, &mime).await
        }
        OpenRouter => {
            let base = base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(normalize_openai_base)
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
            transcribe_via_chat_completions(&app, &base, &api_key, &model, &audio_base64).await
        }
        Gemini => {
            let base = base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());
            transcribe_via_gemini(&app, &base, &api_key, &model, &audio_base64, &mime).await
        }
        Claude => Err(
            "Anthropic has no audio-transcription endpoint. Pick OpenAI, Gemini, OpenRouter or a \
             Compatible provider for audio input."
                .to_string(),
        ),
    }
}

/// OpenAI-style `/audio/transcriptions` (Whisper) path. Streams when the model
/// supports it (`gpt-4o-transcribe` family), otherwise reads a single JSON body.
async fn transcribe_via_openai_endpoint(
    app: &AppHandle,
    base: &str,
    api_key: &str,
    model: &str,
    bytes: Vec<u8>,
    mime: &str,
) -> Result<TranscriptResult, String> {
    // Derive a file extension from the MIME so the API content-sniffs correctly
    // (e.g. "audio/webm;codecs=opus" → "webm", "audio/wav" → "wav").
    let ext = mime
        .rsplit('/')
        .next()
        .and_then(|s| s.split(';').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("wav");
    let filename = format!("audio.{ext}");

    let url = format!("{base}/audio/transcriptions");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)
        .map_err(|e| e.to_string())?;
    // whisper-1 hard-rejects `stream=true` (HTTP 400); the gpt-4o-transcribe
    // family supports it. Only request streaming when the model can do it —
    // everything else returns a single JSON body (handled by the fallback).
    let streamable = !model.to_lowercase().contains("whisper");
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .part("file", part);
    if streamable {
        form = form.text("stream", "true");
    }

    let mut resp = client
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Transcription failed (HTTP {status}): {body}"));
    }

    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false);

    if is_sse {
        // SSE: accumulate bytes, split on newlines, parse `data:` payloads.
        let mut pending = String::new();
        let mut full = String::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            pending.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim_end_matches(['\r', '\n']);
                let data = match line.strip_prefix("data:") {
                    Some(d) => d.trim(),
                    None => continue,
                };
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };
                let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if typ == "transcript.text.done" {
                    if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
                        full = t.to_string();
                    }
                } else if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                    // `transcript.text.delta` (or a gateway variant without a type).
                    full.push_str(delta);
                    emit_delta(app, delta);
                }
            }
        }
        return finish(app, full);
    }

    // Non-streaming fallback: a single `{ "text": "..." }` body.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string();
    emit_delta(app, &text);
    finish(app, text)
}

/// OpenAI-compatible `/chat/completions` path with an `input_audio` content
/// block (OpenRouter, and any chat-only gateway with audio-capable models).
/// Streams `choices[].delta.content`; falls back to the non-streaming body.
async fn transcribe_via_chat_completions(
    app: &AppHandle,
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
) -> Result<TranscriptResult, String> {
    let url = format!("{base}/chat/completions");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "Transcribe the following audio verbatim. Output only the transcription text, with no commentary, labels, or quotation marks." },
                { "type": "input_audio", "input_audio": { "data": audio_base64, "format": "wav" } }
            ]
        }]
    });

    let mut resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Transcription failed (HTTP {status}): {body}"));
    }

    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false);

    if is_sse {
        let mut pending = String::new();
        let mut full = String::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            pending.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim_end_matches(['\r', '\n']);
                let data = match line.strip_prefix("data:") {
                    Some(d) => d.trim(),
                    None => continue,
                };
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };
                if let Some(delta) = v
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_str())
                {
                    full.push_str(delta);
                    emit_delta(app, delta);
                }
            }
        }
        return finish(app, full.trim().to_string());
    }

    // Non-streaming fallback: `choices[0].message.content`.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let text = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    emit_delta(app, &text);
    finish(app, text)
}

/// Gemini native `:streamGenerateContent` path — audio as `inline_data` plus a
/// transcription prompt. Streams `candidates[].content.parts[].text`.
async fn transcribe_via_gemini(
    app: &AppHandle,
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
) -> Result<TranscriptResult, String> {
    // Strip any codec suffix; Gemini wants a bare audio MIME (e.g. "audio/wav").
    let mime_clean = mime.split(';').next().unwrap_or("audio/wav");
    let url = format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": "Transcribe the following audio verbatim. Output only the transcription text, with no commentary, labels, or quotation marks." },
                { "inlineData": { "mimeType": mime_clean, "data": audio_base64 } }
            ]
        }]
    });

    let mut resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Transcription failed (HTTP {status}): {body}"));
    }

    // Extract every `candidates[].content.parts[].text` from a Gemini chunk,
    // emit each as a delta, and append to `full`.
    fn drain_parts(app: &AppHandle, v: &serde_json::Value, full: &mut String) {
        let Some(cands) = v.get("candidates").and_then(|c| c.as_array()) else {
            return;
        };
        for cand in cands {
            let Some(parts) = cand
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            else {
                continue;
            };
            for part in parts {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    full.push_str(t);
                    emit_delta(app, t);
                }
            }
        }
    }

    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false);

    if is_sse {
        let mut pending = String::new();
        let mut full = String::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            pending.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim_end_matches(['\r', '\n']);
                let data = match line.strip_prefix("data:") {
                    Some(d) => d.trim(),
                    None => continue,
                };
                if data.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    drain_parts(app, &v, &mut full);
                }
            }
        }
        return finish(app, full.trim().to_string());
    }

    // Non-streaming fallback: the API may return a JSON array of chunks or a
    // single response object.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let mut full = String::new();
    match &v {
        serde_json::Value::Array(items) => {
            for item in items {
                drain_parts(app, item, &mut full);
            }
        }
        other => drain_parts(app, other, &mut full),
    }
    finish(app, full.trim().to_string())
}
