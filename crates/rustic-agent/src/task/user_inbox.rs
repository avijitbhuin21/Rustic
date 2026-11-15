//! Per-task inbox for user messages sent while a top-level task is running.
//! The executor drains it right after each tool batch (and before ending a
//! turn), so a queued message reaches the model at the next step instead of
//! waiting for the whole turn to finish.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::provider::{ContentBlock, Message, Role};

/// One queued user message; `id` is the frontend queue id echoed back on delivery.
#[derive(Debug, Clone)]
pub struct InboxMessage {
    pub id: String,
    pub text: String,
    /// (media_type, raw bytes)
    pub images: Vec<(String, Vec<u8>)>,
}

static INBOX: LazyLock<Mutex<HashMap<String, Vec<InboxMessage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Queue a message for `task_id`'s running executor.
pub fn push(task_id: &str, msg: InboxMessage) {
    let mut map = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(task_id.to_string()).or_default().push(msg);
}

/// Remove and return everything queued for `task_id`.
pub fn drain(task_id: &str) -> Vec<InboxMessage> {
    let mut map = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    map.remove(task_id).unwrap_or_default()
}

/// Remove a single queued message (user discarded it or sent it another way).
/// Returns true when it was still pending.
pub fn remove(task_id: &str, id: &str) -> bool {
    let mut map = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let Some(list) = map.get_mut(task_id) else {
        return false;
    };
    let before = list.len();
    list.retain(|m| m.id != id);
    let removed = list.len() != before;
    if list.is_empty() {
        map.remove(task_id);
    }
    removed
}

/// Builds the user message for drained inbox entries. Image bytes go to the
/// media store (same path tool attachments use) so history stays slim.
pub fn to_message(msgs: &[InboxMessage]) -> Option<Message> {
    let mut content = Vec::new();
    for m in msgs {
        if !m.text.trim().is_empty() {
            content.push(ContentBlock::Text {
                text: m.text.clone(),
            });
        }
        for (media_type, data) in &m.images {
            use base64::Engine as _;
            let media_type = crate::tools::sniff_image_media_type(data)
                .map(str::to_string)
                .unwrap_or_else(|| media_type.clone());
            content.push(match crate::media_store::store_bytes(data) {
                Some(name) => ContentBlock::Image {
                    media_type,
                    data: String::new(),
                    path: Some(name),
                },
                None => ContentBlock::Image {
                    media_type,
                    data: base64::engine::general_purpose::STANDARD.encode(data),
                    path: None,
                },
            });
        }
    }
    (!content.is_empty()).then_some(Message {
        role: Role::User,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, text: &str) -> InboxMessage {
        InboxMessage {
            id: id.into(),
            text: text.into(),
            images: Vec::new(),
        }
    }

    #[test]
    fn push_drain_remove() {
        push("inbox-t1", msg("a", "first"));
        push("inbox-t1", msg("b", "second"));
        assert!(remove("inbox-t1", "a"));
        assert!(!remove("inbox-t1", "a"));
        let drained = drain("inbox-t1");
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "second");
        assert!(drain("inbox-t1").is_empty());
    }

    #[test]
    fn to_message_joins_text_blocks() {
        let m = to_message(&[msg("a", "one"), msg("b", "two"), msg("c", "  ")]).unwrap();
        assert_eq!(m.content.len(), 2);
        assert!(to_message(&[msg("x", "")]).is_none());
    }
}
