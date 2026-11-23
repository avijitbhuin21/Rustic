//! Inline editor chat (Ctrl+I in the code editor), shared by the desktop
//! (Tauri) and server (Axum) `inline_edit` commands. The caller resolves the
//! provider entry and forwards stream snapshots as events; this module owns
//! the prompt, the reply protocol (first line `EDIT` or `ANSWER`), the
//! streaming one-shot chat call, output cleanup and cancellation.

use crate::config::{AiConfig, ModelCapabilities};
use crate::provider::{
    ContentBlock, Message, ProviderConfig, ProviderStreamEvent, Role, StopReason, StreamCallback,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Event carrying an [`InlineEditDelta`] snapshot while a reply streams.
pub const DELTA_EVENT: &str = "inline-edit-delta";

/// Error returned when the request was cancelled via [`cancel`].
pub const CANCELLED_ERROR: &str = "Inline edit cancelled";

/// Largest selection (in chars) that can be rewritten in one request.
pub const SELECTION_CHAR_CAP: usize = 60_000;
/// How much of the file before / after the selection is sent as context.
pub const PREFIX_CHAR_CAP: usize = 16_000;
pub const SUFFIX_CHAR_CAP: usize = 6_000;

const MAX_OUTPUT_TOKENS: u32 = 16_384;
const DELTA_INTERVAL: Duration = Duration::from_millis(33);
/// A reply with no newline inside this many chars has no marker line.
const MARKER_SCAN_LIMIT: usize = 24;

const SYSTEM_PROMPT: &str = "You are the inline assistant of a code editor. The user selected part of a file (or only placed the cursor) and typed an instruction. You see the text before the selection in <before>, the selection in <selection> (or <cursor/> when nothing is selected) and the text after it in <after>. The tags are delimiters; everything between them is the file's text verbatim.\n\nPick one reply kind:\n- EDIT: the instruction asks to change, fix, refactor, translate, complete or write text in the file.\n- ANSWER: the instruction is a question or asks for an explanation and does not ask for the file to change.\n\nThe first line of your reply must be exactly EDIT or ANSWER and nothing else.\n\nAfter EDIT, output only the text that replaces the selection, or that is inserted at the cursor when nothing is selected. It is spliced into the file exactly as you write it, so:\n- No code fences, no explanation, no commentary.\n- Do not repeat anything from <before> or <after>.\n- Start exactly where the selection starts and stop exactly where it ends. The selection can begin or end in the middle of a line.\n- Keep the indentation of every line (same characters, tabs or spaces, same depth), the quoting style, naming and line endings of the surrounding code.\n- Change only what the instruction requires. Everything else in the selection stays character for character the same.\n\nAfter ANSWER, reply concisely in Markdown. Do not restate the selection unless a short quote is needed.";

/// What the model decided to do with the instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InlineMode {
    Edit,
    Answer,
}

/// Provider + model the request runs on, resolved from `AiConfig`.
pub struct InlineEditTarget {
    /// `ProviderEntry::provider_key()` — decides the adapter routing.
    pub provider_key: String,
    pub model: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub capabilities: ModelCapabilities,
    /// OpenRouter per-model provider allow-list, when restricted.
    pub allowed_providers: Option<Vec<String>>,
}

impl InlineEditTarget {
    /// Resolve the provider entry for `provider_key` and the per-model settings for `model`.
    pub fn resolve(cfg: &AiConfig, provider_key: &str, model: &str) -> Result<Self, String> {
        let model = model.trim();
        if provider_key.trim().is_empty() || model.is_empty() {
            return Err("Pick a model for inline chat first.".to_string());
        }
        let entry = cfg.find_by_key(provider_key.trim()).ok_or_else(|| {
            format!(
                "The provider \"{}\" is no longer connected. Pick another model.",
                provider_key
            )
        })?;
        Ok(Self {
            provider_key: entry.provider_key(),
            model: model.to_string(),
            api_key: entry.api_key.clone(),
            base_url: entry.base_url.clone(),
            capabilities: cfg.capabilities_for(model),
            allowed_providers: cfg.allowed_providers_for(model),
        })
    }
}

/// One inline request as sent by the editor.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineEditInput {
    /// Client-generated id; tags stream events and addresses [`cancel`].
    pub request_id: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    /// File text before the selection (the caller may already window it).
    #[serde(default)]
    pub prefix: String,
    /// Selected text; empty means "insert at the cursor".
    #[serde(default)]
    pub selection: String,
    /// File text after the selection.
    #[serde(default)]
    pub suffix: String,
    pub instruction: String,
}

/// Final result of an inline request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineEditOutput {
    pub mode: InlineMode,
    /// Replacement / insertion text for `Edit`, Markdown for `Answer`.
    pub text: String,
    /// The model stopped at its output limit, so `text` may be incomplete.
    pub truncated: bool,
}

/// Snapshot of the reply so far (the full cleaned text, not an increment).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineEditDelta {
    pub request_id: String,
    pub mode: InlineMode,
    pub text: String,
}

/// Receives stream snapshots; hosts forward them as [`DELTA_EVENT`].
pub type DeltaSink = Arc<dyn Fn(InlineEditDelta) + Send + Sync>;

fn cancel_registry() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cancel the in-flight request with this id. Returns false when it is not running.
pub fn cancel(request_id: &str) -> bool {
    let registry = cancel_registry().lock().unwrap_or_else(|e| e.into_inner());
    match registry.get(request_id) {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// Removes a request's cancel flag from the registry when the request ends.
struct CancelRegistration {
    request_id: String,
}

impl Drop for CancelRegistration {
    fn drop(&mut self) {
        cancel_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.request_id);
    }
}

/// Run one inline request: stream snapshots to `on_delta`, return the cleaned result.
pub async fn run_inline_edit(
    target: InlineEditTarget,
    input: InlineEditInput,
    on_delta: DeltaSink,
) -> Result<InlineEditOutput, String> {
    let instruction = input.instruction.trim().to_string();
    if instruction.is_empty() {
        return Err("Type an instruction first.".to_string());
    }
    if input.request_id.trim().is_empty() {
        return Err("Missing request id.".to_string());
    }
    if target.api_key.trim().is_empty() && target.provider_key != "FreeBuff" {
        return Err("The selected provider has no API key.".to_string());
    }
    if input.selection.chars().count() > SELECTION_CHAR_CAP {
        return Err(format!(
            "The selection is too large for inline chat (over {} characters). Select less, or use the agent panel.",
            SELECTION_CHAR_CAP
        ));
    }

    let selection = input.selection.replace("\r\n", "\n");
    let prefix_full = input.prefix.replace("\r\n", "\n");
    let suffix_full = input.suffix.replace("\r\n", "\n");
    let prefix = tail_chars(&prefix_full, PREFIX_CHAR_CAP);
    let suffix = head_chars(&suffix_full, SUFFIX_CHAR_CAP);
    let user_prompt = build_user_prompt(
        input.path.as_deref(),
        input.language.as_deref(),
        prefix,
        &selection,
        suffix,
        &instruction,
    );
    let fallback = fallback_mode(&instruction);

    let cancel_flag = Arc::new(AtomicBool::new(false));
    cancel_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(input.request_id.clone(), Arc::clone(&cancel_flag));
    let _registration = CancelRegistration {
        request_id: input.request_id.clone(),
    };

    let registered_max = target.capabilities.max_output_tokens;
    let max_tokens = if registered_max > 0 {
        registered_max
    } else {
        crate::model_registry::max_output_tokens(&target.model, 8192)
    }
    .clamp(1024, MAX_OUTPUT_TOKENS);

    let config = ProviderConfig {
        api_key: target.api_key,
        model: target.model,
        max_tokens,
        temperature: 0.2,
        base_url: target.base_url,
        system_prompt: Some(SYSTEM_PROMPT.to_string()),
        thinking_budget: 0,
        context_window: 0,
        condense_threshold: 0,
        web_search_enabled: false,
        web_fetch_enabled: false,
        supports_temperature: target.capabilities.supports_temperature,
        supports_reasoning_effort: target.capabilities.supports_reasoning_effort,
        supports_adaptive_thinking: target.capabilities.supports_adaptive_thinking,
        cancel_token: Some(Arc::clone(&cancel_flag)),
        custom_input_cost: None,
        custom_output_cost: None,
        custom_cache_read_cost: None,
        custom_cache_write_cost: None,
        allowed_providers: target.allowed_providers,
        request_overrides: target.capabilities.request_params.clone(),
    };

    struct StreamState {
        raw: String,
        last_emit: Option<Instant>,
    }
    let stream_state = Arc::new(Mutex::new(StreamState {
        raw: String::new(),
        last_emit: None,
    }));
    let stream_cb: StreamCallback = {
        let stream_state = Arc::clone(&stream_state);
        let request_id = input.request_id.clone();
        let selection = selection.clone();
        Arc::new(move |event| {
            let ProviderStreamEvent::TextDelta(chunk) = event else {
                return;
            };
            let mut st = stream_state.lock().unwrap_or_else(|e| e.into_inner());
            st.raw.push_str(&chunk);
            if st.last_emit.is_some_and(|at| at.elapsed() < DELTA_INTERVAL) {
                return;
            }
            let Some((mode, text)) = parse_reply(&st.raw, &selection, fallback, false) else {
                return;
            };
            st.last_emit = Some(Instant::now());
            drop(st);
            on_delta(InlineEditDelta {
                request_id: request_id.clone(),
                mode,
                text,
            });
        })
    };

    let provider = crate::commit_message::provider_for(&target.provider_key);
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: user_prompt }],
    }];

    // Providers only look at the cancel token when the next chunk arrives, so
    // poll it here too: dropping the future aborts a request that is still
    // waiting for its first byte.
    let chat = provider.chat(&messages, &[], &config, Some(stream_cb));
    tokio::pin!(chat);
    let result = loop {
        tokio::select! {
            r = &mut chat => break r,
            _ = tokio::time::sleep(Duration::from_millis(60)) => {
                if cancel_flag.load(Ordering::SeqCst) {
                    return Err(CANCELLED_ERROR.to_string());
                }
            }
        }
    };
    let response = match result {
        Ok(r) => r,
        Err(_) if cancel_flag.load(Ordering::SeqCst) => return Err(CANCELLED_ERROR.to_string()),
        Err(e) => return Err(e.to_string()),
    };

    let mut raw = String::new();
    for block in &response.content {
        if let ContentBlock::Text { text } = block {
            raw.push_str(text);
        }
    }
    if raw.trim().is_empty() {
        return Err("The model returned an empty reply.".to_string());
    }
    let (mode, text) = parse_reply(&raw, &selection, fallback, true)
        .ok_or_else(|| "The model returned an empty reply.".to_string())?;
    if text.is_empty() && (mode == InlineMode::Answer || selection.is_empty()) {
        return Err("The model returned an empty reply.".to_string());
    }
    Ok(InlineEditOutput {
        mode,
        text,
        truncated: matches!(response.stop_reason, StopReason::MaxTokens),
    })
}

/// Mode assumed when the model omits the marker line: a question is answered, anything else edits.
fn fallback_mode(instruction: &str) -> InlineMode {
    if instruction.trim_end().ends_with('?') {
        InlineMode::Answer
    } else {
        InlineMode::Edit
    }
}

/// The last `cap` chars of `s`, starting at a line boundary when anything was cut.
fn tail_chars(s: &str, cap: usize) -> &str {
    let total = s.chars().count();
    if total <= cap {
        return s;
    }
    let start = s
        .char_indices()
        .nth(total - cap)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let tail = &s[start..];
    match tail.find('\n') {
        Some(nl) if nl + 1 < tail.len() => &tail[nl + 1..],
        _ => tail,
    }
}

/// The first `cap` chars of `s`.
fn head_chars(s: &str, cap: usize) -> &str {
    match s.char_indices().nth(cap) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Assemble the user message: file identity, the three verbatim regions, then the instruction.
fn build_user_prompt(
    path: Option<&str>,
    language: Option<&str>,
    prefix: &str,
    selection: &str,
    suffix: &str,
    instruction: &str,
) -> String {
    let mut out = String::with_capacity(prefix.len() + selection.len() + suffix.len() + 256);
    if let Some(path) = path.map(str::trim).filter(|p| !p.is_empty()) {
        out.push_str("File: ");
        out.push_str(path);
        out.push('\n');
    }
    if let Some(language) = language.map(str::trim).filter(|l| !l.is_empty()) {
        out.push_str("Language: ");
        out.push_str(language);
        out.push('\n');
    }
    out.push_str("\n<before>");
    out.push_str(prefix);
    out.push_str("</before>");
    if selection.is_empty() {
        out.push_str("<cursor/>");
    } else {
        out.push_str("<selection>");
        out.push_str(selection);
        out.push_str("</selection>");
    }
    out.push_str("<after>");
    out.push_str(suffix);
    out.push_str("</after>\n\nInstruction: ");
    out.push_str(instruction);
    out
}

/// Recognise the `EDIT` / `ANSWER` marker line, tolerating decoration such as `**EDIT**`.
fn marker_of(line: &str) -> Option<InlineMode> {
    let word = line.trim().trim_matches(|c: char| !c.is_ascii_alphabetic());
    if word.eq_ignore_ascii_case("EDIT") {
        Some(InlineMode::Edit)
    } else if word.eq_ignore_ascii_case("ANSWER") {
        Some(InlineMode::Answer)
    } else {
        None
    }
}

/// Split a raw reply into its mode and body. `None` while the marker line is still streaming.
fn split_reply(raw: &str, fallback: InlineMode, done: bool) -> Option<(InlineMode, &str)> {
    let lead = raw.trim_start();
    match lead.find('\n') {
        Some(nl) => match marker_of(&lead[..nl]) {
            Some(mode) => Some((mode, &lead[nl + 1..])),
            None => Some((fallback, raw)),
        },
        None if done => match marker_of(lead) {
            Some(mode) => Some((mode, "")),
            None => Some((fallback, raw)),
        },
        None if lead.len() > MARKER_SCAN_LIMIT => Some((fallback, raw)),
        None => None,
    }
}

/// Drop a Markdown code fence wrapped around an edit body, including one that is still streaming.
fn strip_code_fence(body: &str, done: bool) -> &str {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("```") {
        return body;
    }
    let Some(nl) = trimmed.find('\n') else {
        return "";
    };
    let inner = &trimmed[nl + 1..];
    if let Some(before_fence) = inner.trim_end().strip_suffix("```") {
        if before_fence.is_empty() || before_fence.ends_with('\n') {
            return before_fence.strip_suffix('\n').unwrap_or(before_fence);
        }
    }
    if !done {
        let (head, last_line) = match inner.rfind('\n') {
            Some(pos) => (&inner[..pos], &inner[pos + 1..]),
            None => ("", inner),
        };
        if !last_line.is_empty() && last_line.chars().all(|c| c == '`') {
            return head;
        }
    }
    inner
}

/// Leading whitespace of the least-indented non-blank line, or `None` when every line is blank.
fn min_indent(text: &str) -> Option<&str> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| &line[..line.len() - line.trim_start().len()])
        .min_by_key(|indent| indent.chars().count())
}

/// Re-indent a rewrite the model returned fully dedented, using the selection's own indentation.
fn repair_indent(selection: &str, body: &str) -> String {
    let (Some(sel_indent), Some(body_indent)) = (min_indent(selection), min_indent(body)) else {
        return body.to_string();
    };
    if sel_indent.is_empty() || !body_indent.is_empty() {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len() + sel_indent.len() * 8);
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if !line.trim().is_empty() {
            out.push_str(sel_indent);
        }
        out.push_str(line);
    }
    out
}

/// Make the edit end the way the selection does (with or without a final newline).
fn fit_to_selection(selection: &str, body: &str) -> String {
    let mut out = body.to_string();
    if selection.ends_with('\n') {
        if !out.ends_with('\n') {
            out.push('\n');
        }
    } else {
        while out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

/// Turn the raw reply (so far, or complete when `done`) into its mode and cleaned text.
fn parse_reply(
    raw: &str,
    selection: &str,
    fallback: InlineMode,
    done: bool,
) -> Option<(InlineMode, String)> {
    let (mode, body) = split_reply(raw, fallback, done)?;
    let text = match mode {
        InlineMode::Answer => body.trim().to_string(),
        InlineMode::Edit => {
            let body = body.replace("\r\n", "\n");
            // A selection that is itself a fenced block legitimately starts with a fence.
            let unfenced = if selection.trim_start().starts_with("```") {
                body.as_str()
            } else {
                strip_code_fence(&body, done)
            };
            let fitted = fit_to_selection(selection, unfenced);
            if selection.is_empty() {
                fitted
            } else {
                repair_indent(selection, &fitted)
            }
        }
    };
    Some((mode, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(raw: &str, selection: &str) -> String {
        let (mode, text) = parse_reply(raw, selection, InlineMode::Edit, true).unwrap();
        assert_eq!(mode, InlineMode::Edit);
        text
    }

    #[test]
    fn marker_line_selects_mode() {
        let (mode, text) =
            parse_reply("ANSWER\nIt sorts the list.\n", "x", InlineMode::Edit, true).unwrap();
        assert_eq!(mode, InlineMode::Answer);
        assert_eq!(text, "It sorts the list.");
        assert_eq!(edit("EDIT\nlet a = 2;", "let a = 1;"), "let a = 2;");
        assert_eq!(edit("\n**edit**\r\nlet a = 2;", "let a = 1;"), "let a = 2;");
    }

    #[test]
    fn missing_marker_uses_fallback_and_keeps_whole_reply() {
        assert_eq!(
            edit("    let a = 2;\n", "    let a = 1;\n"),
            "    let a = 2;\n"
        );
        assert_eq!(fallback_mode("what does this do?"), InlineMode::Answer);
        assert_eq!(fallback_mode("rename to total"), InlineMode::Edit);
        let (mode, text) =
            parse_reply("It sorts the list.", "x", InlineMode::Answer, true).unwrap();
        assert_eq!(mode, InlineMode::Answer);
        assert_eq!(text, "It sorts the list.");
    }

    #[test]
    fn marker_still_streaming_is_undecided() {
        assert!(parse_reply("ED", "x", InlineMode::Edit, false).is_none());
        assert!(parse_reply("EDIT", "x", InlineMode::Edit, false).is_none());
        let (mode, text) = parse_reply("EDIT\nfoo", "x", InlineMode::Edit, false).unwrap();
        assert_eq!((mode, text.as_str()), (InlineMode::Edit, "foo"));
        // No newline within the scan limit: there is no marker line at all.
        let long = "let total = items.iter().sum::<u32>();";
        let (_, text) = parse_reply(long, "x", InlineMode::Edit, false).unwrap();
        assert_eq!(text, long);
    }

    #[test]
    fn edit_body_keeps_leading_indentation() {
        assert_eq!(
            edit("EDIT\n    foo();\n    bar();\n", "    foo();\n"),
            "    foo();\n    bar();\n"
        );
    }

    #[test]
    fn code_fence_is_stripped() {
        assert_eq!(
            edit("EDIT\n```rust\nlet a = 2;\n```\n", "let a = 1;"),
            "let a = 2;"
        );
        assert_eq!(
            edit("EDIT\n```\nlet a = 2;\n```", "let a = 1;\n"),
            "let a = 2;\n"
        );
        // Backticks inside unfenced code are left alone.
        assert_eq!(
            edit("EDIT\nconst s = `a`;", "const s = 'a';"),
            "const s = `a`;"
        );
    }

    #[test]
    fn fenced_selection_keeps_its_fence() {
        let sel = "```js\nfoo()\n```\n";
        assert_eq!(
            edit("EDIT\n```js\nbar()\n```\n", sel),
            "```js\nbar()\n```\n"
        );
    }

    #[test]
    fn streaming_hides_partial_fences() {
        let partial = |raw: &str| parse_reply(raw, "x", InlineMode::Edit, false).unwrap().1;
        assert_eq!(partial("EDIT\n```ru"), "");
        assert_eq!(partial("EDIT\n```rust\nlet a"), "let a");
        assert_eq!(partial("EDIT\n```rust\nlet a = 2;\n``"), "let a = 2;");
        assert_eq!(partial("EDIT\n```rust\nlet a = 2;\n```"), "let a = 2;");
    }

    #[test]
    fn trailing_newline_follows_the_selection() {
        assert_eq!(edit("EDIT\nfoo\n\n", "bar"), "foo");
        assert_eq!(edit("EDIT\nfoo", "bar\n"), "foo\n");
        assert_eq!(edit("EDIT\nfn x() {}\n", ""), "fn x() {}");
    }

    #[test]
    fn dedented_rewrite_is_reindented() {
        let sel = "\tif a {\n\t\tb();\n\t}\n";
        assert_eq!(
            edit("EDIT\nif a {\n\tb();\n\n\tc();\n}\n", sel),
            "\tif a {\n\t\tb();\n\n\t\tc();\n\t}\n"
        );
        // Already indented, or a selection starting mid-line: untouched.
        assert_eq!(edit("EDIT\n    x();\n", "    y();\n"), "    x();\n");
        assert_eq!(
            edit("EDIT\nx();\n    z();", "y();\n    z();"),
            "x();\n    z();"
        );
    }

    #[test]
    fn context_is_windowed_on_char_boundaries() {
        assert_eq!(tail_chars("ab\ncd\nef", 100), "ab\ncd\nef");
        assert_eq!(tail_chars("ab\ncd\nef", 4), "ef");
        assert_eq!(tail_chars("héllo\nwörld", 5), "wörld");
        assert_eq!(head_chars("héllo", 2), "hé");
        assert_eq!(head_chars("abc", 10), "abc");
    }

    #[test]
    fn prompt_marks_cursor_when_nothing_is_selected() {
        let p = build_user_prompt(
            Some("src/a.rs"),
            Some("rust"),
            "fn a() {\n",
            "",
            "}\n",
            "add a log",
        );
        assert!(p.contains("<before>fn a() {\n</before><cursor/><after>}\n</after>"));
        assert!(p.starts_with("File: src/a.rs\nLanguage: rust\n"));
        let p = build_user_prompt(None, None, "a", "b", "c", "x");
        assert!(p.contains("<before>a</before><selection>b</selection><after>c</after>"));
    }

    #[test]
    fn cancel_only_reaches_registered_requests() {
        assert!(!cancel("inline-edit-test-missing"));
        let flag = Arc::new(AtomicBool::new(false));
        cancel_registry()
            .lock()
            .unwrap()
            .insert("inline-edit-test-live".to_string(), Arc::clone(&flag));
        {
            let _registration = CancelRegistration {
                request_id: "inline-edit-test-live".to_string(),
            };
            assert!(cancel("inline-edit-test-live"));
            assert!(flag.load(Ordering::SeqCst));
        }
        assert!(!cancel("inline-edit-test-live"));
    }
}
