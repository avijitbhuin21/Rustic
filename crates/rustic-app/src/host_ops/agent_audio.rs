//! Speech-to-text for the prompt-box mic, shared by both hosts.
//!
//! Each provider family transcribes through a different mechanism:
//! * **OpenAI / Compatible** → `/audio/transcriptions` (Whisper). The
//!   `gpt-4o-transcribe` family streams `transcript.text.delta` SSE events;
//!   `whisper-1` (and most gateways) return a single `{ "text": ... }` body.
//! * **OpenRouter** → no transcription endpoint, so the clip goes as an
//!   `input_audio` block to `/chat/completions`.
//! * **Gemini** → native `:streamGenerateContent` with `inlineData`.
//!
//! Partial text is reported through `on_delta` as it arrives; hosts forward it
//! as `audio-transcript-delta` and emit `audio-transcript-done` with the
//! returned full transcript.

use base64::Engine;
use rustic_agent::ProviderType;

/// Callback receiving each non-empty partial transcript chunk.
pub type OnDelta<'a> = &'a (dyn Fn(&str) + Send + Sync);

const TRANSCRIBE_PROMPT: &str = "Transcribe the following audio verbatim. Output only the transcription text, with no commentary, labels, or quotation marks.";

fn emit(on_delta: OnDelta<'_>, text: &str) {
    if !text.is_empty() {
        on_delta(text);
    }
}

/// Normalise an OpenAI-compatible API base to its root (no trailing slash, no
/// `/chat/completions` or `/completions` suffix) so a base saved as
/// `.../openai`, `.../v1`, or `.../chat/completions` all resolve correctly.
pub fn normalize_openai_base(raw: &str) -> String {
    raw.trim()
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/completions")
        .trim_end_matches('/')
        .to_string()
}

/// Transcribe a base64-encoded clip with the given provider and return the
/// full transcript, streaming partial text through `on_delta`.
pub async fn transcribe(
    provider_type: ProviderType,
    base_url: Option<String>,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: Option<String>,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    use ProviderType::*;

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
            via_openai_endpoint(
                "https://api.openai.com/v1",
                api_key,
                model,
                bytes,
                &mime,
                on_delta,
            )
            .await
        }
        Compatible => {
            let raw = base_url.as_deref().unwrap_or("").trim();
            if raw.is_empty() {
                return Err("This Compatible provider has no base URL configured.".to_string());
            }
            let base = normalize_openai_base(raw);
            via_openai_endpoint(&base, api_key, model, bytes, &mime, on_delta).await
        }
        OpenRouter => {
            let base = base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(normalize_openai_base)
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
            via_chat_completions(&base, api_key, model, audio_base64, on_delta).await
        }
        Gemini => {
            let base = base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());
            via_gemini(&base, api_key, model, audio_base64, &mime, on_delta).await
        }
        Claude => Err(
            "Anthropic has no audio-transcription endpoint. Pick OpenAI, Gemini, OpenRouter or a \
             Compatible provider for audio input."
                .to_string(),
        ),
        FreeBuff => Err(
            "FreeBuff has no audio-transcription endpoint. Pick OpenAI, Gemini, OpenRouter or a \
             Compatible provider for audio input."
                .to_string(),
        ),
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())
}

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, String> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    Err(format!("Transcription failed (HTTP {status}): {body}"))
}

fn is_sse(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false)
}

/// Reads an SSE body, calling `on_data` with every non-empty `data:` payload.
async fn for_each_sse_data(
    resp: &mut reqwest::Response,
    mut on_data: impl FnMut(&str),
) -> Result<(), String> {
    let mut pending = String::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        pending.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = pending.find('\n') {
            let line: String = pending.drain(..=nl).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if !data.is_empty() {
                on_data(data);
            }
        }
    }
    Ok(())
}

/// OpenAI-style `/audio/transcriptions` (Whisper) path. Streams when the model
/// supports it (`gpt-4o-transcribe` family), otherwise reads a single JSON body.
async fn via_openai_endpoint(
    base: &str,
    api_key: &str,
    model: &str,
    bytes: Vec<u8>,
    mime: &str,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
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
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)
        .map_err(|e| e.to_string())?;
    // whisper-1 hard-rejects `stream=true` (HTTP 400); only request streaming
    // when the model can do it.
    let streamable = !model.to_lowercase().contains("whisper");
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .part("file", part);
    if streamable {
        form = form.text("stream", "true");
    }

    let resp = client()?
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let mut resp = check_status(resp).await?;

    if is_sse(&resp) {
        let mut full = String::new();
        for_each_sse_data(&mut resp, |data| {
            if data == "[DONE]" {
                return;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                return;
            };
            let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if typ == "transcript.text.done" {
                if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
                    full = t.to_string();
                }
            } else if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                // `transcript.text.delta` (or a gateway variant without a type).
                full.push_str(delta);
                emit(on_delta, delta);
            }
        })
        .await?;
        return Ok(full);
    }

    // Non-streaming fallback: a single `{ "text": "..." }` body.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string();
    emit(on_delta, &text);
    Ok(text)
}

/// OpenAI-compatible `/chat/completions` path with an `input_audio` content
/// block (OpenRouter, and any chat-only gateway with audio-capable models).
async fn via_chat_completions(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    let url = format!("{base}/chat/completions");
    let body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": TRANSCRIBE_PROMPT },
                { "type": "input_audio", "input_audio": { "data": audio_base64, "format": "wav" } }
            ]
        }]
    });

    let resp = client()?
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let mut resp = check_status(resp).await?;

    if is_sse(&resp) {
        let mut full = String::new();
        for_each_sse_data(&mut resp, |data| {
            if data == "[DONE]" {
                return;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                return;
            };
            if let Some(delta) = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
            {
                full.push_str(delta);
                emit(on_delta, delta);
            }
        })
        .await?;
        return Ok(full.trim().to_string());
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
    emit(on_delta, &text);
    Ok(text)
}

/// Appends every `candidates[].content.parts[].text` of a Gemini chunk to
/// `full`, reporting each as a delta.
fn drain_gemini_parts(v: &serde_json::Value, full: &mut String, on_delta: OnDelta<'_>) {
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
                emit(on_delta, t);
            }
        }
    }
}

/// Gemini native `:streamGenerateContent` path — audio as `inlineData` plus a
/// transcription prompt.
async fn via_gemini(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    // Strip any codec suffix; Gemini wants a bare audio MIME (e.g. "audio/wav").
    let mime_clean = mime.split(';').next().unwrap_or("audio/wav");
    let url = format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse");
    let body = serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": TRANSCRIBE_PROMPT },
                { "inlineData": { "mimeType": mime_clean, "data": audio_base64 } }
            ]
        }]
    });

    let resp = client()?
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let mut resp = check_status(resp).await?;

    let mut full = String::new();
    if is_sse(&resp) {
        for_each_sse_data(&mut resp, |data| {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                drain_gemini_parts(&v, &mut full, on_delta);
            }
        })
        .await?;
        return Ok(full.trim().to_string());
    }

    // Non-streaming fallback: a JSON array of chunks or a single object.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    match &v {
        serde_json::Value::Array(items) => {
            for item in items {
                drain_gemini_parts(item, &mut full, on_delta);
            }
        }
        other => drain_gemini_parts(other, &mut full, on_delta),
    }
    Ok(full.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_bases() {
        assert_eq!(normalize_openai_base(" https://x/v1/ "), "https://x/v1");
        assert_eq!(
            normalize_openai_base("https://x/v1/chat/completions"),
            "https://x/v1"
        );
        assert_eq!(
            normalize_openai_base("https://x/v1/completions/"),
            "https://x/v1"
        );
    }

    #[test]
    fn drains_gemini_parts_and_reports_deltas() {
        let seen = std::sync::Mutex::new(Vec::<String>::new());
        let cb = |t: &str| seen.lock().unwrap().push(t.to_string());
        let v = serde_json::json!({"candidates":[{"content":{"parts":[{"text":"he"},{"text":"llo"}]}}]});
        let mut full = String::new();
        drain_gemini_parts(&v, &mut full, &cb);
        assert_eq!(full, "hello");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }
}
