//! Slimmed content blocks for the chat transcript the frontend loads.
//!
//! Opening a task used to ship every byte of its history to the UI: base64
//! image payloads, 89 KB tool results, 100 KB thinking blocks. A heavy task was
//! 23 MB of JSON crossing the IPC boundary before a single row rendered, and the
//! virtualized transcript displays a handful of rows at a time — so nearly all
//! of it was wasted.
//!
//! [`slim_content`] rewrites blocks into display stubs: oversized strings are
//! cut at [`BLOCK_TRUNCATE_BYTES`] and image payloads are dropped entirely. Each
//! rewritten block carries `block_index` plus a marker (`truncated` /
//! `payload_deferred`) so the UI can fetch the full block on demand — when the
//! user expands the tool card, or when the image scrolls into view.
//!
//! Blocks come back as `serde_json::Value` rather than `ContentBlock` because the
//! stub markers are display metadata that has no place in the provider-facing
//! enum.

use crate::media_store;
use crate::provider::ContentBlock;
use serde_json::{json, Value};

/// Cut-off for a single block's text payload. Comfortably larger than the
/// ~2 KB of a tool result the UI shows collapsed, small enough that a whole
/// transcript of truncated blocks stays a few hundred KB.
pub const BLOCK_TRUNCATE_BYTES: usize = 24 * 1024;

/// Truncate `s` to at most `BLOCK_TRUNCATE_BYTES`, respecting UTF-8 boundaries.
/// Returns `None` when it already fits.
fn cut(s: &str) -> Option<String> {
    if s.len() <= BLOCK_TRUNCATE_BYTES {
        return None;
    }
    let mut end = BLOCK_TRUNCATE_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].to_string())
}

/// Rewrite one block for display. `index` is its position in the message so the
/// UI can ask for the full version via `get_message_block`. `defer_images`
/// drops image payloads; callers whose messages have no addressable
/// `sort_order` (archived history) must keep them inline instead, since there
/// is nothing for the UI to fetch against.
fn slim_block(index: usize, block: &ContentBlock, defer_images: bool) -> Value {
    let mut value = serde_json::to_value(block).unwrap_or(Value::Null);
    let Some(obj) = value.as_object_mut() else {
        return value;
    };
    obj.insert("block_index".into(), json!(index));

    match block {
        ContentBlock::Text { text } => {
            if let Some(short) = cut(text) {
                obj.insert("text".into(), json!(short));
                obj.insert("truncated".into(), json!(text.len()));
            }
        }
        ContentBlock::Thinking { thinking, .. } => {
            if let Some(short) = cut(thinking) {
                obj.insert("thinking".into(), json!(short));
                obj.insert("truncated".into(), json!(thinking.len()));
            }
        }
        ContentBlock::ToolResult { content, .. } => {
            if let Some(short) = cut(content) {
                obj.insert("content".into(), json!(short));
                obj.insert("truncated".into(), json!(content.len()));
            }
        }
        ContentBlock::ToolUse { input, .. } => {
            // A tool_use's input is usually small, but `create_file` carries a
            // whole file body. Truncating the serialized form would produce
            // invalid JSON the UI can't render, so oversized inputs are
            // replaced wholesale and fetched on expand.
            let encoded = serde_json::to_string(input).unwrap_or_default();
            if encoded.len() > BLOCK_TRUNCATE_BYTES {
                obj.insert("input".into(), json!({}));
                obj.insert("truncated".into(), json!(encoded.len()));
            }
        }
        ContentBlock::Image {
            data,
            path,
            media_type,
        } => {
            if !defer_images {
                // Inline path: the payload has to travel with the transcript,
                // so pull it out of the store if that's where it lives.
                if data.is_empty() {
                    if let Some(b64) = path.as_deref().and_then(media_store::load_base64) {
                        obj.insert("data".into(), json!(b64));
                    }
                }
                return value;
            }
            // The payload never travels with the transcript. `bytes` is the
            // decoded size when the payload is in the store, else the base64
            // length — either way it's only used to size the placeholder.
            let bytes = path
                .as_deref()
                .and_then(media_store::payload_len)
                .unwrap_or(data.len() as u64);
            obj.insert("data".into(), json!(""));
            obj.insert("payload_deferred".into(), json!(true));
            obj.insert("bytes".into(), json!(bytes));
            obj.insert("media_type".into(), json!(media_type));
        }
        _ => {}
    }
    value
}

/// Slim a whole message's content for the transcript DTO, deferring image
/// payloads to `get_message_block`.
pub fn slim_content(content: &[ContentBlock]) -> Vec<Value> {
    content
        .iter()
        .enumerate()
        .map(|(i, b)| slim_block(i, b, true))
        .collect()
}

/// Slim text payloads but keep images inline — for messages the UI cannot
/// issue a follow-up fetch for (archived generations carry no `sort_order`).
pub fn slim_content_inline_images(content: &[ContentBlock]) -> Vec<Value> {
    content
        .iter()
        .enumerate()
        .map(|(i, b)| slim_block(i, b, false))
        .collect()
}

/// A sub-agent record's replay payload, trimmed for the list view.
pub struct SlimReplay {
    pub output_text: String,
    pub tool_calls_json: String,
    /// True when anything was cut — the UI fetches the full replay when the
    /// user actually opens that sub-agent.
    pub truncated: bool,
}

/// Trim a sub-agent record's stored replay. `get_subagent_records` runs on
/// every task open and returns one row per child, each holding the child's
/// whole accumulated text plus every tool call it made with full outputs — for
/// a task that spawned a dozen children that is tens of MB nobody looks at
/// until they click into a specific child.
pub fn slim_subagent_replay(output_text: &str, tool_calls_json: &str) -> SlimReplay {
    let mut truncated = false;
    let output = match cut(output_text) {
        Some(short) => {
            truncated = true;
            short
        }
        None => output_text.to_string(),
    };

    let calls = match serde_json::from_str::<Vec<Value>>(tool_calls_json) {
        Ok(calls) => calls,
        // Unparseable — pass it through untouched rather than lose the replay.
        Err(_) => {
            return SlimReplay {
                output_text: output,
                tool_calls_json: tool_calls_json.to_string(),
                truncated,
            }
        }
    };
    let mut slim = Vec::with_capacity(calls.len());
    for mut call in calls {
        if let Some(obj) = call.as_object_mut() {
            if let Some(result) = obj.get("result").and_then(Value::as_str) {
                if let Some(short) = cut(result) {
                    let full_len = result.len();
                    obj.insert("result".into(), json!(short));
                    obj.insert("truncated".into(), json!(full_len));
                    truncated = true;
                }
            }
            // A tool_use's input can't be cut without producing invalid JSON,
            // so an oversized one is dropped wholesale (same rule as the
            // transcript's tool_use blocks).
            let input_len = obj
                .get("input")
                .map(|v| serde_json::to_string(v).unwrap_or_default().len())
                .unwrap_or(0);
            if input_len > BLOCK_TRUNCATE_BYTES {
                obj.insert("input".into(), json!({}));
                obj.insert("truncated".into(), json!(input_len));
                truncated = true;
            }
        }
        slim.push(call);
    }
    SlimReplay {
        output_text: output,
        tool_calls_json: serde_json::to_string(&slim).unwrap_or_else(|_| "[]".to_string()),
        truncated,
    }
}

/// The full block at `index`, with its image payload hydrated from the media
/// store — what `get_message_block` hands back when the UI needs the real
/// thing. `None` when the index is out of range.
pub fn full_block(content: &[ContentBlock], index: usize) -> Option<Value> {
    let block = content.get(index)?;
    let mut one = [block.clone()];
    media_store::hydrate(&mut one);
    let mut value = serde_json::to_value(&one[0]).ok()?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("block_index".into(), json!(index));
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_blocks_pass_through_untouched() {
        let blocks = vec![ContentBlock::Text {
            text: "hello".into(),
        }];
        let slim = slim_content(&blocks);
        assert_eq!(slim[0]["text"], json!("hello"));
        assert!(slim[0].get("truncated").is_none());
        assert_eq!(slim[0]["block_index"], json!(0));
    }

    #[test]
    fn oversized_tool_result_is_cut_and_marked() {
        let big = "x".repeat(BLOCK_TRUNCATE_BYTES * 2);
        let blocks = vec![ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: big.clone(),
            is_error: false,
        }];
        let slim = slim_content(&blocks);
        assert_eq!(
            slim[0]["content"].as_str().unwrap().len(),
            BLOCK_TRUNCATE_BYTES
        );
        assert_eq!(slim[0]["truncated"], json!(big.len()));
    }

    #[test]
    fn image_payload_is_never_shipped() {
        let blocks = vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "AAAABBBB".into(),
            path: None,
        }];
        let slim = slim_content(&blocks);
        assert_eq!(slim[0]["data"], json!(""));
        assert_eq!(slim[0]["payload_deferred"], json!(true));
        assert_eq!(slim[0]["bytes"], json!(8));
    }

    #[test]
    fn full_block_returns_the_untruncated_payload() {
        let big = "y".repeat(BLOCK_TRUNCATE_BYTES * 2);
        let blocks = vec![ContentBlock::Text { text: big.clone() }];
        let full = full_block(&blocks, 0).unwrap();
        assert_eq!(full["text"].as_str().unwrap().len(), big.len());
        assert!(full_block(&blocks, 7).is_none());
    }

    #[test]
    fn truncation_never_splits_a_multibyte_char() {
        // 'é' is 2 bytes; the cut lands mid-character without a boundary walk.
        let s = "é".repeat(BLOCK_TRUNCATE_BYTES);
        let blocks = vec![ContentBlock::Text { text: s }];
        let slim = slim_content(&blocks);
        assert!(slim[0]["text"].as_str().is_some());
    }
}
