use ropey::Rope;
use serde::{Deserialize, Serialize};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use super::languages::LanguageRegistry;

/// The highlight names we recognize, in order.
/// These map to CSS classes: token-keyword, token-string, etc.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.method",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// Maps highlight name to a simplified token class for CSS.
fn highlight_to_token_class(name: &str) -> &'static str {
    if name.starts_with("keyword") {
        "keyword"
    } else if name.starts_with("string") {
        "string"
    } else if name.starts_with("comment") {
        "comment"
    } else if name.starts_with("function") {
        "function"
    } else if name.starts_with("type") {
        "type"
    } else if name.starts_with("variable") {
        "variable"
    } else if name.starts_with("number") || name.starts_with("constant") {
        "number"
    } else if name.starts_with("operator") {
        "operator"
    } else if name.starts_with("punctuation") {
        "punctuation"
    } else if name.starts_with("property") {
        "variable"
    } else if name.starts_with("attribute") || name.starts_with("tag") {
        "keyword"
    } else if name.starts_with("constructor") {
        "type"
    } else if name.starts_with("label") {
        "variable"
    } else {
        "variable"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub start_col: usize,
    pub end_col: usize,
    pub highlight_class: String,
}

pub type HighlightedLine = Vec<Span>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedLine {
    pub line_number: usize,
    pub text: String,
    pub spans: Vec<Span>,
}

pub struct SyntaxHighlighter {
    config: HighlightConfiguration,
}

impl SyntaxHighlighter {
    pub fn new(language_name: &str) -> Option<Self> {
        let language = LanguageRegistry::get_language(language_name)?;
        let query = LanguageRegistry::get_highlight_query(language_name)?;

        let mut config = HighlightConfiguration::new(language, language_name, query, "", "").ok()?;
        config.configure(HIGHLIGHT_NAMES);

        Some(Self { config })
    }

    /// Highlight a range of lines from a rope.
    /// Returns RenderedLine objects with text and syntax spans.
    pub fn highlight_lines(
        &self,
        rope: &Rope,
        start_line: usize,
        end_line: usize,
    ) -> Vec<RenderedLine> {
        let end_line = end_line.min(rope.len_lines());
        let source = rope.to_string();
        let source_bytes = source.as_bytes();

        let mut highlighter = Highlighter::new();
        let highlights = match highlighter.highlight(&self.config, source_bytes, None, |_| None) {
            Ok(h) => h,
            Err(_) => {
                // Fallback: return lines without highlighting
                return (start_line..end_line)
                    .map(|i| {
                        let text = rope
                            .line(i)
                            .to_string()
                            .trim_end_matches('\n')
                            .trim_end_matches('\r')
                            .to_string();
                        RenderedLine {
                            line_number: i + 1,
                            text,
                            spans: Vec::new(),
                        }
                    })
                    .collect();
            }
        };

        // Collect all highlight events into per-line spans
        let line_count = rope.len_lines();
        let mut line_spans: Vec<Vec<Span>> = vec![Vec::new(); line_count];
        let mut current_highlight: Option<usize> = None;

        for event in highlights {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Some(highlight_idx) = current_highlight {
                        let class_name =
                            highlight_to_token_class(HIGHLIGHT_NAMES[highlight_idx]);

                        // Map byte range to lines/cols
                        let start_line_idx = byte_to_line(rope, start);
                        let end_line_idx = byte_to_line(rope, end.saturating_sub(1).max(start));

                        for line_idx in start_line_idx..=end_line_idx {
                            if line_idx >= line_count {
                                break;
                            }
                            let line_start_byte = rope.char_to_byte(rope.line_to_char(line_idx));
                            let line_end_byte = if line_idx + 1 < line_count {
                                rope.char_to_byte(rope.line_to_char(line_idx + 1))
                            } else {
                                rope.len_bytes()
                            };

                            let span_start = if start > line_start_byte {
                                start - line_start_byte
                            } else {
                                0
                            };
                            let span_end = if end < line_end_byte {
                                end - line_start_byte
                            } else {
                                line_end_byte - line_start_byte
                            };

                            // Convert byte offsets to char columns for the line
                            let line_str = rope.line(line_idx).to_string();
                            let start_col = byte_offset_to_col(&line_str, span_start);
                            let end_col = byte_offset_to_col(&line_str, span_end);

                            if start_col < end_col {
                                line_spans[line_idx].push(Span {
                                    start_col,
                                    end_col,
                                    highlight_class: class_name.to_string(),
                                });
                            }
                        }
                    }
                }
                Ok(HighlightEvent::HighlightStart(h)) => {
                    current_highlight = Some(h.0);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    current_highlight = None;
                }
                Err(_) => break,
            }
        }

        // Build rendered lines for requested range
        (start_line..end_line)
            .map(|i| {
                let text = rope
                    .line(i)
                    .to_string()
                    .trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_string();
                let mut spans = if i < line_spans.len() {
                    line_spans[i].clone()
                } else {
                    Vec::new()
                };
                // Sort spans by start_col
                spans.sort_by_key(|s| s.start_col);
                RenderedLine {
                    line_number: i + 1,
                    text,
                    spans,
                }
            })
            .collect()
    }
}

fn byte_to_line(rope: &Rope, byte_offset: usize) -> usize {
    let byte_offset = byte_offset.min(rope.len_bytes());
    let char_idx = rope.byte_to_char(byte_offset);
    rope.char_to_line(char_idx)
}

fn byte_offset_to_col(line: &str, byte_offset: usize) -> usize {
    let byte_offset = byte_offset.min(line.len());
    line[..byte_offset].chars().count()
}
