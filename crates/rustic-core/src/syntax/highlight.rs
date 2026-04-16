use regex::Regex;
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
        "property"
    } else if name.starts_with("attribute") {
        "attribute"
    } else if name.starts_with("tag") {
        "tag"
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

struct TreeSitterEngine {
    config: HighlightConfiguration,
    /// Injection configs for embedded languages (e.g. CSS/JS inside HTML)
    injection_configs: HashMap<String, HighlightConfiguration>,
}

enum HighlightEngine {
    TreeSitter(TreeSitterEngine),
    Markdown(MarkdownHighlighter),
    Generic(GenericHighlighter),
}

pub struct SyntaxHighlighter {
    engine: HighlightEngine,
    /// Cached highlighted lines — populated by `ensure_highlighted()`.
    cached_lines: Vec<RenderedLine>,
}

impl SyntaxHighlighter {
    /// Try to create a Tree-sitter backed highlighter. Returns None only if
    /// we want the caller to decide (kept for backwards compat).
    pub fn new(language_name: &str) -> Option<Self> {
        // Markdown gets a dedicated regex-based highlighter for richer styling
        if language_name == "markdown" {
            return Some(Self {
                engine: HighlightEngine::Markdown(MarkdownHighlighter::new()),
                cached_lines: Vec::new(),
            });
        }

        let language = LanguageRegistry::get_language(language_name)?;
        let query = LanguageRegistry::get_highlight_query(language_name)?;
        let injection_query = LanguageRegistry::get_injection_query(language_name).unwrap_or("");

        let mut config =
            HighlightConfiguration::new(language, language_name, query, injection_query, "").ok()?;
        config.configure(HIGHLIGHT_NAMES);

        // Build injection configs for languages that embed others (e.g. HTML → CSS/JS)
        let mut injection_configs = HashMap::new();
        if language_name == "html" {
            for inject_lang in &["css", "javascript"] {
                if let Some(inj_lang) = LanguageRegistry::get_language(inject_lang) {
                    if let Some(inj_query) = LanguageRegistry::get_highlight_query(inject_lang) {
                        if let Ok(mut inj_config) = HighlightConfiguration::new(
                            inj_lang,
                            *inject_lang,
                            inj_query,
                            "",
                            "",
                        ) {
                            inj_config.configure(HIGHLIGHT_NAMES);
                            injection_configs.insert(inject_lang.to_string(), inj_config);
                        }
                    }
                }
            }
        }

        Some(Self {
            engine: HighlightEngine::TreeSitter(TreeSitterEngine {
                config,
                injection_configs,
            }),
            cached_lines: Vec::new(),
        })
    }

    /// Create a generic regex-based fallback highlighter.
    /// Always succeeds — used when no Tree-sitter grammar is available.
    pub fn new_generic() -> Self {
        Self {
            engine: HighlightEngine::Generic(GenericHighlighter::new()),
            cached_lines: Vec::new(),
        }
    }

    /// Returns true if the highlight cache is populated.
    pub fn is_cached(&self) -> bool {
        !self.cached_lines.is_empty()
    }

    /// Invalidate the highlight cache. Call after any buffer edit.
    pub fn invalidate_cache(&mut self) {
        self.cached_lines.clear();
    }

    /// Return a range of highlighted lines from the cache.
    /// Returns None if cache is not populated.
    pub fn get_cached_range(&self, start_line: usize, end_line: usize) -> Option<Vec<RenderedLine>> {
        if self.cached_lines.is_empty() {
            return None;
        }
        let start = start_line.min(self.cached_lines.len());
        let end = end_line.min(self.cached_lines.len());
        Some(self.cached_lines[start..end].to_vec())
    }

    /// Perform the full parse and cache all highlighted lines.
    /// No-op if cache is already populated.
    pub fn ensure_highlighted(&mut self, rope: &Rope) {
        if !self.cached_lines.is_empty() {
            return;
        }
        self.cached_lines = match &self.engine {
            HighlightEngine::TreeSitter(engine) => treesitter_highlight(engine, rope),
            HighlightEngine::Markdown(md) => md.highlight(rope),
            HighlightEngine::Generic(generic) => generic.highlight(rope),
        };
    }

    /// Highlight only a specific line range. Does a full Tree-sitter parse
    /// (necessary for correctness) but only builds RenderedLine data for
    /// the requested range, avoiding the cost of constructing 10K+ lines
    /// when only ~100 are needed. Returns None if no engine is available.
    pub fn highlight_range(&self, rope: &Rope, start_line: usize, end_line: usize) -> Vec<RenderedLine> {
        match &self.engine {
            HighlightEngine::TreeSitter(engine) => treesitter_highlight_range(engine, rope, start_line, end_line),
            HighlightEngine::Markdown(md) => {
                let all = md.highlight(rope);
                let start = start_line.min(all.len());
                let end = end_line.min(all.len());
                all[start..end].to_vec()
            }
            HighlightEngine::Generic(generic) => {
                let all = generic.highlight(rope);
                let start = start_line.min(all.len());
                let end = end_line.min(all.len());
                all[start..end].to_vec()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tree-sitter highlighting
// ---------------------------------------------------------------------------

fn treesitter_highlight(engine: &TreeSitterEngine, rope: &Rope) -> Vec<RenderedLine> {
    let line_count = rope.len_lines();
    let source = rope.to_string();
    let source_bytes = source.as_bytes();

    let mut highlighter = Highlighter::new();
    let highlights = match highlighter.highlight(&engine.config, source_bytes, None, |lang_name| {
        engine.injection_configs.get(lang_name)
    }) {
        Ok(h) => h,
        Err(_) => {
            return plain_lines(rope, line_count);
        }
    };

    let mut line_spans: Vec<Vec<Span>> = vec![Vec::new(); line_count];
    let mut current_highlight: Option<usize> = None;

    for event in highlights {
        match event {
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(highlight_idx) = current_highlight {
                    let class_name = highlight_to_token_class(HIGHLIGHT_NAMES[highlight_idx]);

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

    (0..line_count)
        .map(|i| {
            let text = rope
                .line(i)
                .to_string()
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string();
            let mut spans = if i < line_spans.len() {
                std::mem::take(&mut line_spans[i])
            } else {
                Vec::new()
            };
            spans.sort_by_key(|s| s.start_col);
            RenderedLine {
                line_number: i + 1,
                text,
                spans,
            }
        })
        .collect()
}

/// Like `treesitter_highlight` but only builds RenderedLine data for the
/// requested range [start_line, end_line). Still does a full Tree-sitter
/// parse (required for correctness with injections), but skips span
/// collection for lines outside the range, making it much faster for
/// viewport-only highlighting of large files.
fn treesitter_highlight_range(
    engine: &TreeSitterEngine,
    rope: &Rope,
    start_line: usize,
    end_line: usize,
) -> Vec<RenderedLine> {
    let line_count = rope.len_lines();
    let start_line = start_line.min(line_count);
    let end_line = end_line.min(line_count);
    if start_line >= end_line {
        return Vec::new();
    }

    let source = rope.to_string();
    let source_bytes = source.as_bytes();

    let mut highlighter = Highlighter::new();
    let highlights = match highlighter.highlight(&engine.config, source_bytes, None, |lang_name| {
        engine.injection_configs.get(lang_name)
    }) {
        Ok(h) => h,
        Err(_) => {
            // Fall back to plain text for the requested range
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

    // Only collect spans for lines in the requested range
    let range_size = end_line - start_line;
    let mut line_spans: Vec<Vec<Span>> = vec![Vec::new(); range_size];
    let mut current_highlight: Option<usize> = None;

    for event in highlights {
        match event {
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(highlight_idx) = current_highlight {
                    let start_line_idx = byte_to_line(rope, start);
                    let end_line_idx = byte_to_line(rope, end.saturating_sub(1).max(start));

                    // Skip spans entirely outside our range
                    if end_line_idx < start_line || start_line_idx >= end_line {
                        continue;
                    }

                    let class_name = highlight_to_token_class(HIGHLIGHT_NAMES[highlight_idx]);

                    let effective_start = start_line_idx.max(start_line);
                    let effective_end = (end_line_idx + 1).min(end_line);

                    for line_idx in effective_start..effective_end {
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

                        let line_str = rope.line(line_idx).to_string();
                        let start_col = byte_offset_to_col(&line_str, span_start);
                        let end_col = byte_offset_to_col(&line_str, span_end);

                        if start_col < end_col {
                            let idx = line_idx - start_line;
                            line_spans[idx].push(Span {
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

    (0..range_size)
        .map(|i| {
            let line_idx = start_line + i;
            let text = rope
                .line(line_idx)
                .to_string()
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string();
            let mut spans = std::mem::take(&mut line_spans[i]);
            spans.sort_by_key(|s| s.start_col);
            RenderedLine {
                line_number: line_idx + 1,
                text,
                spans,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Markdown highlighting
// ---------------------------------------------------------------------------

struct MarkdownHighlighter {
    // ATX headings: # Heading
    re_atx_heading: Regex,
    // Code blocks: ``` or ~~~
    re_code_fence: Regex,
    // Inline code: `code`
    re_inline_code: Regex,
    // Bold: **text** or __text__
    re_bold: Regex,
    // Italic: *text* or _text_
    re_italic: Regex,
    // Bold+italic: ***text*** or ___text___
    re_bold_italic: Regex,
    // Strikethrough: ~~text~~
    re_strikethrough: Regex,
    // Links: [text](url) or [text][ref]
    re_link: Regex,
    // Images: ![alt](url)
    re_image: Regex,
    // Reference-style link definitions: [ref]: url
    re_link_def: Regex,
    // Block quotes: > text
    re_blockquote: Regex,
    // Unordered list markers: - * +
    re_ul_marker: Regex,
    // Ordered list markers: 1. 2)
    re_ol_marker: Regex,
    // Horizontal rules: --- *** ___
    re_hr: Regex,
    // HTML tags
    re_html_tag: Regex,
    // Task list items: - [ ] or - [x]
    re_task: Regex,
    // Footnotes: [^ref]
    re_footnote: Regex,
    // Autolinks and bare URLs
    re_autolink: Regex,
}

impl MarkdownHighlighter {
    fn new() -> Self {
        Self {
            re_atx_heading: Regex::new(r"^(#{1,6})\s+(.*)$").unwrap(),
            re_code_fence: Regex::new(r"^(\s*)(```|~~~)(.*)$").unwrap(),
            // No backreferences — separate patterns for each backtick count
            re_inline_code: Regex::new(r"(``)(.+?)``|`([^`]+)`").unwrap(),
            // Bold+italic: ***text*** or ___text___
            re_bold_italic: Regex::new(r"\*\*\*(.+?)\*\*\*|___(.+?)___").unwrap(),
            // Bold: **text** or __text__
            re_bold: Regex::new(r"\*\*(.+?)\*\*|__(.+?)__").unwrap(),
            // Italic: *text* or _text_ (not preceded/followed by same char)
            re_italic: Regex::new(r"\*([^\s*][^*]*?)\*|(?:^|[\s(])_([^\s_][^_]*?)_(?:$|[\s)])").unwrap(),
            re_strikethrough: Regex::new(r"(~~)(.+?)(~~)").unwrap(),
            re_link: Regex::new(r"\[([^\]]*)\]\(([^)]*)\)").unwrap(),
            re_image: Regex::new(r"!\[([^\]]*)\]\(([^)]*)\)").unwrap(),
            re_link_def: Regex::new(r"^\[([^\]]+)\]:\s+(.+)$").unwrap(),
            re_blockquote: Regex::new(r"^(\s*>+)\s?(.*)$").unwrap(),
            re_ul_marker: Regex::new(r"^(\s*)([-*+])\s").unwrap(),
            re_ol_marker: Regex::new(r"^(\s*)(\d+[.)]) ").unwrap(),
            // HR: three or more of the same character (-, *, _)
            re_hr: Regex::new(r"^(\s*)(---+|\*\*\*+|___+)\s*$").unwrap(),
            re_html_tag: Regex::new(r"</?[a-zA-Z][a-zA-Z0-9]*[^>]*>").unwrap(),
            re_task: Regex::new(r"^(\s*[-*+]\s+)(\[[ xX]\])").unwrap(),
            re_footnote: Regex::new(r"\[\^([^\]]+)\]").unwrap(),
            re_autolink: Regex::new(r"<(https?://[^>]+)>|(?:^|\s)(https?://\S+)").unwrap(),
        }
    }

    fn highlight(&self, rope: &Rope) -> Vec<RenderedLine> {
        let line_count = rope.len_lines();
        let mut result = Vec::with_capacity(line_count);
        let mut in_code_block = false;

        for i in 0..line_count {
            let text = rope
                .line(i)
                .to_string()
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string();

            let spans = self.highlight_line(&text, &mut in_code_block);
            result.push(RenderedLine {
                line_number: i + 1,
                text,
                spans,
            });
        }
        result
    }

    fn highlight_line(&self, line: &str, in_code_block: &mut bool) -> Vec<Span> {
        let len = line.chars().count();
        if len == 0 {
            return Vec::new();
        }

        let byte_to_char = |byte_off: usize| -> usize {
            line[..byte_off.min(line.len())].chars().count()
        };

        // Code fence toggle
        if self.re_code_fence.is_match(line) {
            *in_code_block = !*in_code_block;
            // Color the fence line itself as code
            return vec![Span {
                start_col: 0,
                end_col: len,
                highlight_class: "md-code".to_string(),
            }];
        }

        // Inside a fenced code block — entire line is code
        if *in_code_block {
            return vec![Span {
                start_col: 0,
                end_col: len,
                highlight_class: "md-code".to_string(),
            }];
        }

        // Indented code block (4 spaces or 1 tab)
        if line.starts_with("    ") || line.starts_with('\t') {
            // Only treat as code if it looks like code (not a list continuation)
            let trimmed = line.trim_start();
            if !trimmed.starts_with('-')
                && !trimmed.starts_with('*')
                && !trimmed.starts_with('+')
                && !trimmed.starts_with('>')
            {
                return vec![Span {
                    start_col: 0,
                    end_col: len,
                    highlight_class: "md-code".to_string(),
                }];
            }
        }

        let mut claimed = vec![false; len];
        let mut spans: Vec<Span> = Vec::new();

        // Horizontal rule — whole line
        if self.re_hr.is_match(line) {
            return vec![Span {
                start_col: 0,
                end_col: len,
                highlight_class: "md-hr".to_string(),
            }];
        }

        // ATX heading: # Heading
        if let Some(caps) = self.re_atx_heading.captures(line) {
            let marker = caps.get(1).unwrap();
            let level = marker.as_str().len(); // 1-6
            let class = format!("md-h{}", level);

            // Color the # marker
            let mc_start = byte_to_char(marker.start());
            let mc_end = byte_to_char(marker.end());
            spans.push(Span {
                start_col: mc_start,
                end_col: mc_end,
                highlight_class: "md-heading-marker".to_string(),
            });
            claim(&mut claimed, mc_start, mc_end);

            // Color the heading text
            if let Some(text_match) = caps.get(2) {
                let tc_start = byte_to_char(text_match.start());
                let tc_end = byte_to_char(text_match.end());
                if tc_start < tc_end {
                    spans.push(Span {
                        start_col: tc_start,
                        end_col: tc_end,
                        highlight_class: class,
                    });
                    claim(&mut claimed, tc_start, tc_end);
                }
            }
            // Headings don't need further inline processing
            spans.sort_by_key(|s| s.start_col);
            return spans;
        }

        // Block quote
        if let Some(caps) = self.re_blockquote.captures(line) {
            let marker = caps.get(1).unwrap();
            let sc = byte_to_char(marker.start());
            let ec = byte_to_char(marker.end());
            spans.push(Span {
                start_col: sc,
                end_col: ec,
                highlight_class: "md-blockquote-marker".to_string(),
            });
            claim(&mut claimed, sc, ec);

            // Rest of line gets blockquote styling
            if ec < len {
                spans.push(Span {
                    start_col: ec,
                    end_col: len,
                    highlight_class: "md-blockquote".to_string(),
                });
                claim(&mut claimed, ec, len);
            }
            spans.sort_by_key(|s| s.start_col);
            return spans;
        }

        // Task list items: - [ ] or - [x]
        if let Some(caps) = self.re_task.captures(line) {
            if let Some(checkbox) = caps.get(2) {
                let sc = byte_to_char(checkbox.start());
                let ec = byte_to_char(checkbox.end());
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "md-task".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // List markers
        if let Some(caps) = self.re_ul_marker.captures(line) {
            if let Some(marker) = caps.get(2) {
                let sc = byte_to_char(marker.start());
                let ec = byte_to_char(marker.end());
                if !claimed[sc] {
                    spans.push(Span {
                        start_col: sc,
                        end_col: ec,
                        highlight_class: "md-list-marker".to_string(),
                    });
                    claim(&mut claimed, sc, ec);
                }
            }
        } else if let Some(caps) = self.re_ol_marker.captures(line) {
            if let Some(marker) = caps.get(2) {
                let sc = byte_to_char(marker.start());
                let ec = byte_to_char(marker.end());
                if !claimed[sc] {
                    spans.push(Span {
                        start_col: sc,
                        end_col: ec,
                        highlight_class: "md-list-marker".to_string(),
                    });
                    claim(&mut claimed, sc, ec);
                }
            }
        }

        // Link definitions: [ref]: url
        if let Some(caps) = self.re_link_def.captures(line) {
            if let Some(label) = caps.get(1) {
                let sc = byte_to_char(label.start().saturating_sub(1)); // include [
                let ec = byte_to_char(label.end() + 1); // include ]
                spans.push(Span {
                    start_col: sc,
                    end_col: ec.min(len),
                    highlight_class: "md-link-text".to_string(),
                });
                claim(&mut claimed, sc, ec.min(len));
            }
            if let Some(url) = caps.get(2) {
                let sc = byte_to_char(url.start());
                let ec = byte_to_char(url.end());
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "md-link-url".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
            spans.sort_by_key(|s| s.start_col);
            return spans;
        }

        // --- Inline elements (order matters for overlapping patterns) ---

        // Images: ![alt](url) — before links since links are a subset
        for caps in self.re_image.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                // Color the whole thing, then overlay parts
                if let Some(alt) = caps.get(1) {
                    let as_ = byte_to_char(alt.start());
                    let ae = byte_to_char(alt.end());
                    if as_ < ae {
                        spans.push(Span {
                            start_col: sc,
                            end_col: as_,
                            highlight_class: "md-image-marker".to_string(),
                        });
                        spans.push(Span {
                            start_col: as_,
                            end_col: ae,
                            highlight_class: "md-link-text".to_string(),
                        });
                    }
                }
                if let Some(url) = caps.get(2) {
                    let us = byte_to_char(url.start());
                    let ue = byte_to_char(url.end());
                    // bracket+paren between alt and url
                    let alt_end = caps.get(1).map(|a| byte_to_char(a.end())).unwrap_or(sc);
                    spans.push(Span {
                        start_col: alt_end,
                        end_col: us,
                        highlight_class: "md-image-marker".to_string(),
                    });
                    spans.push(Span {
                        start_col: us,
                        end_col: ue,
                        highlight_class: "md-link-url".to_string(),
                    });
                    spans.push(Span {
                        start_col: ue,
                        end_col: ec,
                        highlight_class: "md-image-marker".to_string(),
                    });
                }
                claim(&mut claimed, sc, ec);
            }
        }

        // Links: [text](url)
        for caps in self.re_link.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                // [
                spans.push(Span {
                    start_col: sc,
                    end_col: sc + 1,
                    highlight_class: "punctuation".to_string(),
                });
                if let Some(text) = caps.get(1) {
                    let ts = byte_to_char(text.start());
                    let te = byte_to_char(text.end());
                    if ts < te {
                        spans.push(Span {
                            start_col: ts,
                            end_col: te,
                            highlight_class: "md-link-text".to_string(),
                        });
                    }
                }
                if let Some(url) = caps.get(2) {
                    let us = byte_to_char(url.start());
                    let ue = byte_to_char(url.end());
                    // ](
                    let text_end = caps.get(1).map(|t| byte_to_char(t.end())).unwrap_or(sc + 1);
                    spans.push(Span {
                        start_col: text_end,
                        end_col: us,
                        highlight_class: "punctuation".to_string(),
                    });
                    spans.push(Span {
                        start_col: us,
                        end_col: ue,
                        highlight_class: "md-link-url".to_string(),
                    });
                    // )
                    spans.push(Span {
                        start_col: ue,
                        end_col: ec,
                        highlight_class: "punctuation".to_string(),
                    });
                }
                claim(&mut claimed, sc, ec);
            }
        }

        // Autolinks: <https://...> or bare URLs
        for caps in self.re_autolink.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc.min(len - 1)] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "md-link-url".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // Footnotes: [^ref]
        for caps in self.re_footnote.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "md-footnote".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // Inline code: `code` (before bold/italic since backticks take precedence)
        for caps in self.re_inline_code.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "md-code".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // Bold+italic: ***text*** or ___text___
        for caps in self.re_bold_italic.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                let inner = caps.get(1).or_else(|| caps.get(2));
                spans.push(Span {
                    start_col: sc,
                    end_col: sc + 3,
                    highlight_class: "md-bold-italic-marker".to_string(),
                });
                if let Some(inner) = inner {
                    let is_ = byte_to_char(inner.start());
                    let ie = byte_to_char(inner.end());
                    spans.push(Span {
                        start_col: is_,
                        end_col: ie,
                        highlight_class: "md-bold-italic".to_string(),
                    });
                }
                spans.push(Span {
                    start_col: ec - 3,
                    end_col: ec,
                    highlight_class: "md-bold-italic-marker".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // Bold: **text** or __text__
        for caps in self.re_bold.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                let inner = caps.get(1).or_else(|| caps.get(2));
                spans.push(Span {
                    start_col: sc,
                    end_col: sc + 2,
                    highlight_class: "md-bold-marker".to_string(),
                });
                if let Some(inner) = inner {
                    let is_ = byte_to_char(inner.start());
                    let ie = byte_to_char(inner.end());
                    spans.push(Span {
                        start_col: is_,
                        end_col: ie,
                        highlight_class: "md-bold".to_string(),
                    });
                }
                spans.push(Span {
                    start_col: ec - 2,
                    end_col: ec,
                    highlight_class: "md-bold-marker".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // Italic: *text* or _text_
        for caps in self.re_italic.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                let inner = caps.get(1).or_else(|| caps.get(2));
                spans.push(Span {
                    start_col: sc,
                    end_col: sc + 1,
                    highlight_class: "md-italic-marker".to_string(),
                });
                if let Some(inner) = inner {
                    let is_ = byte_to_char(inner.start());
                    let ie = byte_to_char(inner.end());
                    spans.push(Span {
                        start_col: is_,
                        end_col: ie,
                        highlight_class: "md-italic".to_string(),
                    });
                }
                spans.push(Span {
                    start_col: ec - 1,
                    end_col: ec,
                    highlight_class: "md-italic-marker".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // Strikethrough: ~~text~~
        for caps in self.re_strikethrough.captures_iter(line) {
            let full = caps.get(0).unwrap();
            let sc = byte_to_char(full.start());
            let ec = byte_to_char(full.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: sc + 2,
                    highlight_class: "md-strikethrough-marker".to_string(),
                });
                if let Some(inner) = caps.get(2) {
                    let is_ = byte_to_char(inner.start());
                    let ie = byte_to_char(inner.end());
                    spans.push(Span {
                        start_col: is_,
                        end_col: ie,
                        highlight_class: "md-strikethrough".to_string(),
                    });
                }
                spans.push(Span {
                    start_col: ec - 2,
                    end_col: ec,
                    highlight_class: "md-strikethrough-marker".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // HTML tags
        for m in self.re_html_tag.find_iter(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "md-html".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        spans.sort_by_key(|s| s.start_col);
        spans
    }
}

// ---------------------------------------------------------------------------
// Generic regex-based highlighting (fallback for unknown languages)
// ---------------------------------------------------------------------------

struct GenericHighlighter {
    // Comment patterns: # // -- ; (full-line or trailing)
    re_comment: Regex,
    // Double-quoted strings
    re_double_string: Regex,
    // Single-quoted strings
    re_single_string: Regex,
    // Numbers (integers, floats, hex)
    re_number: Regex,
    // Boolean / null-like constants
    re_boolean: Regex,
    // Key in key=value or key: value (at line start)
    re_key: Regex,
    // Brackets
    re_bracket: Regex,
    // Section headers [section] or [section.sub]
    re_section: Regex,
}

impl GenericHighlighter {
    fn new() -> Self {
        Self {
            re_comment: Regex::new(r"(?:#|//|--|;).*$").unwrap(),
            re_double_string: Regex::new(r#""(?:[^"\\]|\\.)*""#).unwrap(),
            re_single_string: Regex::new(r"'(?:[^'\\]|\\.)*'").unwrap(),
            re_number: Regex::new(r"\b(?:0[xX][0-9a-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?\d+)?)\b").unwrap(),
            re_boolean: Regex::new(r"\b(?:true|false|True|False|TRUE|FALSE|yes|no|Yes|No|YES|NO|null|nil|None|NULL)\b").unwrap(),
            re_key: Regex::new(r"^[ \t]*([A-Za-z_][\w.\-/]*)[ \t]*[:=]").unwrap(),
            re_bracket: Regex::new(r"[\[\](){}]").unwrap(),
            re_section: Regex::new(r"^\s*\[[\w.\-/\s]+\]").unwrap(),
        }
    }

    fn highlight(&self, rope: &Rope) -> Vec<RenderedLine> {
        let line_count = rope.len_lines();
        let mut result = Vec::with_capacity(line_count);
        let mut in_multiline_string = false;

        for i in 0..line_count {
            let text = rope
                .line(i)
                .to_string()
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string();

            let spans = self.highlight_line(&text, &mut in_multiline_string);
            result.push(RenderedLine {
                line_number: i + 1,
                text,
                spans,
            });
        }
        result
    }

    fn highlight_line(&self, line: &str, in_multiline_string: &mut bool) -> Vec<Span> {
        let len = line.chars().count();
        if len == 0 {
            return Vec::new();
        }

        // Track which character positions are already claimed
        let mut claimed = vec![false; len];
        let mut spans: Vec<Span> = Vec::new();

        // Helper: map byte offset to char column
        let byte_to_char = |byte_off: usize| -> usize {
            line[..byte_off.min(line.len())].chars().count()
        };

        // Handle multiline string continuation (triple-quoted)
        if *in_multiline_string {
            if let Some(end) = line.find("\"\"\"") {
                let end_col = byte_to_char(end + 3);
                spans.push(Span {
                    start_col: 0,
                    end_col,
                    highlight_class: "string".to_string(),
                });
                claim(&mut claimed, 0, end_col);
                *in_multiline_string = false;
            } else {
                spans.push(Span {
                    start_col: 0,
                    end_col: len,
                    highlight_class: "string".to_string(),
                });
                return spans;
            }
        }

        // Check for triple-quoted string start
        if let Some(start) = line.find("\"\"\"") {
            let start_col = byte_to_char(start);
            if !claimed[start_col] {
                // Check for closing on same line
                if let Some(end) = line[start + 3..].find("\"\"\"") {
                    let end_col = byte_to_char(start + 3 + end + 3);
                    spans.push(Span {
                        start_col,
                        end_col,
                        highlight_class: "string".to_string(),
                    });
                    claim(&mut claimed, start_col, end_col);
                } else {
                    spans.push(Span {
                        start_col,
                        end_col: len,
                        highlight_class: "string".to_string(),
                    });
                    claim(&mut claimed, start_col, len);
                    *in_multiline_string = true;
                    return spans;
                }
            }
        }

        // 1. Comments (highest priority — once we hit a comment, the rest is comment)
        if let Some(m) = self.re_comment.find(line) {
            let start_col = byte_to_char(m.start());
            // Make sure the comment marker isn't inside a string we already found
            if start_col < len && !claimed[start_col] {
                spans.push(Span {
                    start_col,
                    end_col: len,
                    highlight_class: "comment".to_string(),
                });
                claim(&mut claimed, start_col, len);
            }
        }

        // 2. Strings (double-quoted)
        for m in self.re_double_string.find_iter(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "string".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // 3. Strings (single-quoted)
        for m in self.re_single_string.find_iter(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "string".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // 4. Section headers [section]
        if let Some(m) = self.re_section.find(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "keyword".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // 5. Key in key=value / key: value
        if let Some(caps) = self.re_key.captures(line) {
            if let Some(key_match) = caps.get(1) {
                let sc = byte_to_char(key_match.start());
                let ec = byte_to_char(key_match.end());
                if sc < len && !claimed[sc] {
                    spans.push(Span {
                        start_col: sc,
                        end_col: ec,
                        highlight_class: "variable".to_string(),
                    });
                    claim(&mut claimed, sc, ec);
                }
            }
        }

        // 6. Booleans / null
        for m in self.re_boolean.find_iter(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "number".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // 7. Numbers
        for m in self.re_number.find_iter(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "number".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        // 8. Brackets
        for m in self.re_bracket.find_iter(line) {
            let sc = byte_to_char(m.start());
            let ec = byte_to_char(m.end());
            if sc < len && !claimed[sc] {
                spans.push(Span {
                    start_col: sc,
                    end_col: ec,
                    highlight_class: "punctuation".to_string(),
                });
                claim(&mut claimed, sc, ec);
            }
        }

        spans.sort_by_key(|s| s.start_col);
        spans
    }
}

/// Mark character positions as claimed so later patterns don't overlap.
fn claim(claimed: &mut [bool], start: usize, end: usize) {
    for c in claimed.iter_mut().take(end).skip(start) {
        *c = true;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn plain_lines(rope: &Rope, line_count: usize) -> Vec<RenderedLine> {
    (0..line_count)
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
        .collect()
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
