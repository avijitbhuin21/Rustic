use crate::provider::{ContentBlock, Message};

/// Outcome of a history-repair pass, reported back to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    pub stubbed: usize,
    pub targeted: bool,
}

/// Extracts the (message, block) indices from a provider error path like "messages.47.content.1.image.source".
fn parse_error_path(error: &str) -> Option<(usize, usize)> {
    let start = error.find("messages.")?;
    let rest = &error[start + "messages.".len()..];
    let msg_digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if msg_digits.is_empty() {
        return None;
    }
    let rest = &rest[msg_digits.len()..];
    let rest = rest.strip_prefix(".content.")?;
    let blk_digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if blk_digits.is_empty() {
        return None;
    }
    Some((msg_digits.parse().ok()?, blk_digits.parse().ok()?))
}

fn stub_text(error: &str) -> String {
    let short: String = error.chars().take(300).collect();
    format!(
        "[Content removed during error recovery — the provider rejected it with: {}]",
        short
    )
}

/// Repairs a task history that a provider deterministically rejects (4xx): stubs the
/// offending block when the error names it, otherwise stubs all image blocks.
pub fn repair_history_for_provider_error(messages: &mut [Message], error: &str) -> RepairReport {
    // Targeted pass: the Anthropic error path indexes the API request's
    // messages array, which usually lines up 1:1 with our history (the system
    // prompt is a separate field; only stripped-empty messages shift indices).
    // Only accept the target when it points at a non-Text block — a Text hit
    // means the indices are misaligned, so fall through to the generic pass.
    if let Some((mi, bi)) = parse_error_path(error) {
        if let Some(block) = messages.get_mut(mi).and_then(|m| m.content.get_mut(bi)) {
            if !matches!(block, ContentBlock::Text { .. }) {
                *block = ContentBlock::Text {
                    text: stub_text(error),
                };
                return RepairReport {
                    stubbed: 1,
                    targeted: true,
                };
            }
        }
    }

    // Generic pass: binary payloads (images) are by far the most common cause
    // of deterministic 4xx rejections — stub them all.
    let mut stubbed = 0;
    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            if matches!(block, ContentBlock::Image { .. }) {
                *block = ContentBlock::Text {
                    text: stub_text(error),
                };
                stubbed += 1;
            }
        }
    }
    RepairReport {
        stubbed,
        targeted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Role;

    fn img() -> ContentBlock {
        ContentBlock::Image {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        }
    }

    fn text(t: &str) -> ContentBlock {
        ContentBlock::Text { text: t.into() }
    }

    fn msg(role: Role, content: Vec<ContentBlock>) -> Message {
        Message { role, content }
    }

    #[test]
    fn parses_anthropic_error_path() {
        let err = r#"Claude API error 400 Bad Request: {"message":"messages.47.content.1.image.source.base64.media_type: mismatch"}"#;
        assert_eq!(parse_error_path(err), Some((47, 1)));
    }

    #[test]
    fn parse_returns_none_without_path() {
        assert_eq!(parse_error_path("OpenAI error: invalid request"), None);
    }

    #[test]
    fn targeted_repair_stubs_named_block() {
        let mut messages = vec![
            msg(Role::User, vec![text("hi")]),
            msg(Role::User, vec![text("see image"), img()]),
        ];
        let report = repair_history_for_provider_error(
            &mut messages,
            "error: messages.1.content.1.image bad media type",
        );
        assert_eq!(
            report,
            RepairReport {
                stubbed: 1,
                targeted: true
            }
        );
        assert!(matches!(messages[1].content[1], ContentBlock::Text { .. }));
        // The untouched text block survives.
        assert!(matches!(
            &messages[1].content[0],
            ContentBlock::Text { text } if text == "see image"
        ));
    }

    #[test]
    fn misaligned_target_falls_back_to_generic() {
        // Path points at a Text block — indices misaligned; generic pass
        // stubs the actual image elsewhere.
        let mut messages = vec![
            msg(Role::User, vec![text("hi"), text("there")]),
            msg(Role::User, vec![img()]),
        ];
        let report =
            repair_history_for_provider_error(&mut messages, "messages.0.content.1.image broken");
        assert_eq!(
            report,
            RepairReport {
                stubbed: 1,
                targeted: false
            }
        );
        assert!(matches!(messages[1].content[0], ContentBlock::Text { .. }));
        assert!(matches!(
            &messages[0].content[1],
            ContentBlock::Text { text } if text == "there"
        ));
    }

    #[test]
    fn generic_repair_stubs_all_images() {
        let mut messages = vec![
            msg(Role::User, vec![img(), text("a")]),
            msg(Role::Assistant, vec![text("b")]),
            msg(Role::User, vec![img()]),
        ];
        let report = repair_history_for_provider_error(&mut messages, "some unhelpful 400");
        assert_eq!(
            report,
            RepairReport {
                stubbed: 2,
                targeted: false
            }
        );
        assert!(messages
            .iter()
            .flat_map(|m| &m.content)
            .all(|b| !matches!(b, ContentBlock::Image { .. })));
    }

    #[test]
    fn no_images_and_no_path_stubs_nothing() {
        let mut messages = vec![msg(Role::User, vec![text("plain")])];
        let report = repair_history_for_provider_error(&mut messages, "opaque provider error");
        assert_eq!(
            report,
            RepairReport {
                stubbed: 0,
                targeted: false
            }
        );
    }
}
