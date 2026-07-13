use crate::provider::{ContentBlock, Message};

/// Outcome of a history-repair pass, reported back to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    pub stubbed: usize,
    pub targeted: bool,
}

/// Extracts the (message, block, kind) from a provider error path like
/// "messages.47.content.1.image.source" — kind is the path segment right
/// after the block index ("image", "tool_use", "thinking", …) when present.
fn parse_error_path(error: &str) -> Option<(usize, usize, Option<String>)> {
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
    let after = &rest[blk_digits.len()..];
    let kind = after.strip_prefix('.').map(|k| {
        k.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
    });
    let kind = kind.filter(|k| !k.is_empty());
    Some((msg_digits.parse().ok()?, blk_digits.parse().ok()?, kind))
}

/// Wire-format type name of a block, matching the segment Anthropic uses in
/// its error paths (mirrors the serde renames on [`ContentBlock`]).
fn block_kind(block: &ContentBlock) -> &'static str {
    match block {
        ContentBlock::Text { .. } => "text",
        ContentBlock::ToolUse { .. } => "tool_use",
        ContentBlock::ToolResult { .. } => "tool_result",
        ContentBlock::Thinking { .. } => "thinking",
        ContentBlock::RedactedThinking { .. } => "redacted_thinking",
        ContentBlock::Image { .. } => "image",
        ContentBlock::ModelSwitch { .. } => "model_switch",
    }
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
    // messages array, which usually lines up 1:1 with our history — but not
    // always: the wire drops ModelSwitch markers, skips messages emptied by
    // orphan-stripping, and reorders tool_results ahead of text, all of which
    // shift indices. When the path names the block's TYPE we verify it before
    // stubbing; on a mismatch we look for the single block of that type in
    // the named message (covers block-index shifts), and only then fall back
    // to the generic pass. A Text hit is always treated as misaligned.
    if let Some((mi, bi, kind)) = parse_error_path(error) {
        if let Some(block) = messages.get_mut(mi).and_then(|m| m.content.get_mut(bi)) {
            let kind_ok = kind.as_deref().is_none_or(|k| k == block_kind(block));
            if kind_ok && !matches!(block, ContentBlock::Text { .. }) {
                *block = ContentBlock::Text {
                    text: stub_text(error),
                };
                return RepairReport {
                    stubbed: 1,
                    targeted: true,
                };
            }
        }
        // Exact index missed or wrong type — if the error names a kind and the
        // named message holds exactly ONE block of that kind, that's the
        // offender with a shifted block index.
        if let (Some(k), Some(msg)) = (kind.as_deref(), messages.get_mut(mi)) {
            let mut matching = msg
                .content
                .iter_mut()
                .filter(|b| block_kind(b) == k && !matches!(b, ContentBlock::Text { .. }));
            if let (Some(block), None) = (matching.next(), matching.next()) {
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
        assert_eq!(
            parse_error_path(err),
            Some((47, 1, Some("image".to_string())))
        );
    }

    #[test]
    fn parses_path_without_kind() {
        let err = "messages.5.content.2: field required";
        assert_eq!(parse_error_path(err), Some((5, 2, None)));
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

    #[test]
    fn kind_mismatch_at_exact_index_recovers_via_single_block_of_kind() {
        // Wire block index shifted by one (e.g. a filtered ModelSwitch marker):
        // the path names content.0.image but our index 0 is the tool_use. The
        // message holds exactly one image — stub that, not the tool_use.
        let mut messages = vec![msg(
            Role::User,
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "ok".into(),
                    is_error: false,
                },
                img(),
            ],
        )];
        let report = repair_history_for_provider_error(
            &mut messages,
            "messages.0.content.0.image.source: invalid base64",
        );
        assert_eq!(
            report,
            RepairReport {
                stubbed: 1,
                targeted: true
            }
        );
        assert!(matches!(messages[0].content[1], ContentBlock::Text { .. }));
        assert!(matches!(
            messages[0].content[0],
            ContentBlock::ToolResult { .. }
        ));
    }

    #[test]
    fn kind_mismatch_with_ambiguous_candidates_falls_back_to_generic() {
        // Named message holds TWO images and the exact index points at a
        // tool_use — ambiguous, so the generic pass stubs all images instead
        // of guessing.
        let mut messages = vec![msg(
            Role::User,
            vec![
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "x".into(),
                    input: serde_json::json!({}),
                    thought_signature: None,
                },
                img(),
                img(),
            ],
        )];
        let report = repair_history_for_provider_error(
            &mut messages,
            "messages.0.content.0.image.source: invalid",
        );
        assert_eq!(
            report,
            RepairReport {
                stubbed: 2,
                targeted: false
            }
        );
        assert!(matches!(
            messages[0].content[0],
            ContentBlock::ToolUse { .. }
        ));
    }

    #[test]
    fn wrong_kind_at_exact_index_never_stubs_innocent_block() {
        // Path names a tool_use but the block at that index is an image and
        // the message has no tool_use at all — the image must NOT be stubbed
        // by the targeted pass (generic pass may still handle images, which
        // is correct behavior for image blocks).
        let mut messages = vec![msg(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "hmm".into(),
                signature: Some("sig".into()),
                duration_secs: None,
            }],
        )];
        let report = repair_history_for_provider_error(
            &mut messages,
            "messages.0.content.0.tool_use.id: invalid",
        );
        // No tool_use anywhere and no images — nothing gets stubbed.
        assert_eq!(
            report,
            RepairReport {
                stubbed: 0,
                targeted: false
            }
        );
        assert!(matches!(
            messages[0].content[0],
            ContentBlock::Thinking { .. }
        ));
    }
}
