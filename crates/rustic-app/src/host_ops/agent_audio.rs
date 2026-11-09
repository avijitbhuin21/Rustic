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

const TRANSCRIBE_PROMPT: &str = "Transcribe the following audio verbatim. Respond with a JSON object of the form {\"transcript\": \"<the exact words spoken>\"} and nothing else — no commentary, labels, or markdown. Use an empty string if nothing is spoken.";

/// JSON schema the audio-input LLM must answer with (one `transcript` string).
fn transcript_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "transcript": { "type": "string", "description": "Verbatim transcription of the audio." } },
        "required": ["transcript"],
        "additionalProperties": false
    })
}

/// Dedicated speech-to-text model ids (vs. general audio-input LLMs) by name.
fn is_transcription_model_name(model: &str) -> bool {
    let m = model.to_lowercase();
    [
        "whisper",
        "transcribe",
        "stt",
        "chirp",
        "universal-",
        "speech-to-text",
        "voxtral-mini-transcribe",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Audio container name (`wav`, `mp3`, …) the `input_audio.format` field expects.
fn audio_format(mime: &str) -> String {
    let sub = mime
        .split(';')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    match sub.as_str() {
        "" | "x-wav" | "wave" | "vnd.wave" => "wav".to_string(),
        "mpeg" => "mp3".to_string(),
        "x-m4a" | "mp4" => "m4a".to_string(),
        other => other.to_string(),
    }
}

/// Incrementally pulls the `transcript` string out of a streamed JSON reply,
/// falling back to passing text through verbatim when the model ignored the
/// requested format.
#[derive(Default)]
struct TranscriptExtractor {
    raw: String,
    scanned: usize,
    mode: ExtractMode,
    out: String,
    pending_escape: String,
}

#[derive(Default, PartialEq)]
enum ExtractMode {
    #[default]
    Undecided,
    SeekingKey,
    InString,
    Done,
    Plain,
}

impl TranscriptExtractor {
    /// Feed one streamed chunk; returns the newly decoded transcript text.
    fn push(&mut self, chunk: &str) -> String {
        self.raw.push_str(chunk);
        let before = self.out.len();
        if self.mode == ExtractMode::Undecided {
            match self.raw.trim_start().chars().next() {
                None => return String::new(),
                Some('{') | Some('`') => self.mode = ExtractMode::SeekingKey,
                Some(_) => {
                    self.mode = ExtractMode::Plain;
                    self.out.push_str(self.raw.trim_start());
                    self.scanned = self.raw.len();
                    return self.out[before..].to_string();
                }
            }
        }
        match self.mode {
            ExtractMode::Plain => {
                self.out.push_str(&self.raw[self.scanned..]);
                self.scanned = self.raw.len();
            }
            ExtractMode::SeekingKey => self.seek_key(),
            ExtractMode::InString => self.read_string(),
            _ => {}
        }
        self.out[before..].to_string()
    }

    fn seek_key(&mut self) {
        let Some(k) = self.raw.find("\"transcript\"") else {
            return;
        };
        let rest = &self.raw[k + "\"transcript\"".len()..];
        let trimmed = rest.trim_start();
        let Some(after_colon) = trimmed.strip_prefix(':') else {
            return;
        };
        let after_ws = after_colon.trim_start();
        if !after_ws.starts_with('"') {
            return;
        }
        self.scanned = self.raw.len() - after_ws.len() + 1;
        self.mode = ExtractMode::InString;
        self.read_string();
    }

    fn read_string(&mut self) {
        let tail: String = self.raw[self.scanned..].to_string();
        self.scanned = self.raw.len();
        for c in tail.chars() {
            if !self.pending_escape.is_empty() {
                self.pending_escape.push(c);
                let esc = &self.pending_escape;
                if esc.starts_with("\\u") {
                    if esc.len() < 6 {
                        continue;
                    }
                    if let Some(ch) = u32::from_str_radix(&esc[2..6], 16)
                        .ok()
                        .and_then(char::from_u32)
                    {
                        self.out.push(ch);
                    }
                } else {
                    self.out.push(match c {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        'b' | 'f' => ' ',
                        'u' => continue,
                        other => other,
                    });
                }
                self.pending_escape.clear();
                continue;
            }
            match c {
                '\\' => self.pending_escape.push(c),
                '"' => {
                    self.mode = ExtractMode::Done;
                    return;
                }
                other => self.out.push(other),
            }
        }
    }

    /// Final transcript once the stream ends.
    fn finish(self) -> String {
        match self.mode {
            ExtractMode::InString | ExtractMode::Done | ExtractMode::Plain => {
                self.out.trim().to_string()
            }
            ExtractMode::Undecided | ExtractMode::SeekingKey => {
                let body = self
                    .raw
                    .trim()
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();
                serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| {
                        v.get("transcript")
                            .and_then(|t| t.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_else(|| self.raw.trim().to_string())
            }
        }
    }
}

/// Feed `chunk` to the extractor and forward any decoded transcript text.
fn feed(ex: &mut TranscriptExtractor, chunk: &str, on_delta: OnDelta<'_>) {
    let d = ex.push(chunk);
    emit(on_delta, &d);
}

/// Whether a request error means the model/provider rejected structured output.
fn rejects_structured_output(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("http 400")
        && [
            "response_format",
            "json_schema",
            "responseschema",
            "response_schema",
            "responsemimetype",
            "structured",
            "parameter",
            "no endpoints found",
        ]
        .iter()
        .any(|k| e.contains(k))
}

type SttCache = std::sync::Mutex<Option<(std::time::Instant, std::collections::HashSet<String>)>>;
static OPENROUTER_STT_MODELS: std::sync::OnceLock<SttCache> = std::sync::OnceLock::new();

/// Whether an OpenRouter model is a dedicated speech-to-text model (served by
/// `/audio/transcriptions`) rather than an audio-input LLM; list cached 30 min.
async fn openrouter_is_transcription_model(base: &str, api_key: &str, model: &str) -> bool {
    let cache = OPENROUTER_STT_MODELS.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(g) = cache.lock() {
        if let Some((at, set)) = g.as_ref() {
            if at.elapsed() < std::time::Duration::from_secs(30 * 60) {
                return set.contains(model);
            }
        }
    }
    let fetched = async {
        let resp = client()
            .ok()?
            .get(format!("{base}/models"))
            .query(&[("output_modalities", "transcription")])
            .bearer_auth(api_key)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        Some(
            v.get("data")?
                .as_array()?
                .iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                .collect::<std::collections::HashSet<_>>(),
        )
    }
    .await;
    match fetched {
        Some(set) if !set.is_empty() => {
            let hit = set.contains(model);
            if let Ok(mut g) = cache.lock() {
                *g = Some((std::time::Instant::now(), set));
            }
            hit
        }
        _ => is_transcription_model_name(model),
    }
}

/// OpenRouter `/audio/transcriptions`: JSON body with base64 `input_audio`, returns `{ "text" }`.
async fn via_openrouter_stt(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    let body = serde_json::json!({
        "model": model,
        "input_audio": { "data": audio_base64, "format": audio_format(mime) }
    });
    let resp = client()?
        .post(format!("{base}/audio/transcriptions"))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let resp = check_status(resp).await?;
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    emit(on_delta, &text);
    Ok(text)
}

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
        OpenAi if !is_transcription_model_name(model) => {
            via_chat_completions(
                "https://api.openai.com/v1",
                api_key,
                model,
                audio_base64,
                &mime,
                on_delta,
            )
            .await
        }
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
            if openrouter_is_transcription_model(&base, api_key, model).await {
                via_openrouter_stt(&base, api_key, model, audio_base64, &mime, on_delta).await
            } else {
                via_chat_completions(&base, api_key, model, audio_base64, &mime, on_delta).await
            }
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
/// block (OpenRouter, OpenAI audio models, chat-only gateways). Requests a
/// `{"transcript"}` JSON reply; retries without the schema if it's rejected.
async fn via_chat_completions(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    match chat_completions_once(base, api_key, model, audio_base64, mime, true, on_delta).await {
        Err(e) if rejects_structured_output(&e) => {
            chat_completions_once(base, api_key, model, audio_base64, mime, false, on_delta).await
        }
        other => other,
    }
}

/// One chat-completions transcription request, optionally with a JSON-schema response format.
async fn chat_completions_once(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
    structured: bool,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    let url = format!("{base}/chat/completions");
    let mut body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": TRANSCRIBE_PROMPT },
                { "type": "input_audio", "input_audio": { "data": audio_base64, "format": audio_format(mime) } }
            ]
        }]
    });
    if structured {
        body["response_format"] = serde_json::json!({
            "type": "json_schema",
            "json_schema": { "name": "transcription", "strict": true, "schema": transcript_schema() }
        });
    }

    let resp = client()?
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let mut resp = check_status(resp).await?;

    let mut ex = TranscriptExtractor::default();
    if is_sse(&resp) {
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
                feed(&mut ex, delta, on_delta);
            }
        })
        .await?;
        return Ok(ex.finish());
    }

    // Non-streaming fallback: `choices[0].message.content`.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    feed(&mut ex, content, on_delta);
    Ok(ex.finish())
}

/// Passes every `candidates[].content.parts[].text` of a Gemini chunk to `sink`.
fn drain_gemini_parts(v: &serde_json::Value, sink: &mut dyn FnMut(&str)) {
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
                sink(t);
            }
        }
    }
}

/// Gemini native `:streamGenerateContent` path — audio as `inlineData` plus a
/// transcription prompt, answered as `{"transcript"}` JSON (schema dropped on rejection).
async fn via_gemini(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    match gemini_once(base, api_key, model, audio_base64, mime, true, on_delta).await {
        Err(e) if rejects_structured_output(&e) => {
            gemini_once(base, api_key, model, audio_base64, mime, false, on_delta).await
        }
        other => other,
    }
}

/// One Gemini transcription request, optionally constrained to the transcript JSON schema.
async fn gemini_once(
    base: &str,
    api_key: &str,
    model: &str,
    audio_base64: &str,
    mime: &str,
    structured: bool,
    on_delta: OnDelta<'_>,
) -> Result<String, String> {
    // Strip any codec suffix; Gemini wants a bare audio MIME (e.g. "audio/wav").
    let mime_clean = mime.split(';').next().unwrap_or("audio/wav");
    let url = format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse");
    let mut body = serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": TRANSCRIBE_PROMPT },
                { "inlineData": { "mimeType": mime_clean, "data": audio_base64 } }
            ]
        }]
    });
    if structured {
        body["generationConfig"] = serde_json::json!({
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": { "transcript": { "type": "STRING" } },
                "required": ["transcript"]
            }
        });
    }

    let resp = client()?
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let mut resp = check_status(resp).await?;

    let mut ex = TranscriptExtractor::default();
    if is_sse(&resp) {
        for_each_sse_data(&mut resp, |data| {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                drain_gemini_parts(&v, &mut |t| feed(&mut ex, t, on_delta));
            }
        })
        .await?;
        return Ok(ex.finish());
    }

    // Non-streaming fallback: a JSON array of chunks or a single object.
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let mut sink = |t: &str| feed(&mut ex, t, on_delta);
    match &v {
        serde_json::Value::Array(items) => {
            for item in items {
                drain_gemini_parts(item, &mut sink);
            }
        }
        other => drain_gemini_parts(other, &mut sink),
    }
    Ok(ex.finish())
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
        let mut seen = Vec::<String>::new();
        let v = serde_json::json!({"candidates":[{"content":{"parts":[{"text":"he"},{"text":"llo"}]}}]});
        drain_gemini_parts(&v, &mut |t| seen.push(t.to_string()));
        assert_eq!(seen.concat(), "hello");
        assert_eq!(seen.len(), 2);
    }

    /// Feed `chunks` one by one, returning (concatenated deltas, final transcript).
    fn run_extractor(chunks: &[&str]) -> (String, String) {
        let mut ex = TranscriptExtractor::default();
        let deltas: String = chunks.iter().map(|c| ex.push(c)).collect();
        (deltas, ex.finish())
    }

    #[test]
    fn extracts_streamed_json_transcript_with_escapes() {
        let (d, f) = run_extractor(&[
            "{\"tran",
            "script\" : \"He said \\\"hi\\",
            "\" \\u00e9",
            "t\\nok\"}",
        ]);
        assert_eq!(d, "He said \"hi\" ét\nok");
        assert_eq!(f, "He said \"hi\" ét\nok");
    }

    #[test]
    fn extracts_fenced_json_and_plain_text() {
        let (_, f) = run_extractor(&["```json\n{\"transcript\": \"hello\"}\n```"]);
        assert_eq!(f, "hello");
        let (d, f) = run_extractor(&["  hello ", "world"]);
        assert_eq!(d, "hello world");
        assert_eq!(f, "hello world");
    }

    #[test]
    fn classifies_transcription_models_and_formats() {
        assert!(is_transcription_model_name("openai/whisper-large-v3"));
        assert!(is_transcription_model_name("gpt-4o-mini-transcribe"));
        assert!(!is_transcription_model_name("google/gemini-2.5-flash"));
        assert!(!is_transcription_model_name("gpt-4o-audio-preview"));
        assert_eq!(audio_format("audio/wav"), "wav");
        assert_eq!(audio_format("audio/mpeg"), "mp3");
        assert_eq!(audio_format("audio/webm;codecs=opus"), "webm");
    }
}
