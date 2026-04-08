/// Built-in code formatter that handles indentation and basic formatting
/// for all supported languages. No external dependencies required.
///
/// Strategy per language family:
/// - C-like (JS, TS, Rust, Go, Java, C, C++, CSS, JSON, etc.): bracket-nesting based indentation
/// - Python: colon-based block detection, preserves relative indentation within blocks
/// - HTML/XML: tag-nesting based indentation
/// - Fallback: bracket-nesting (works for most languages)

/// Format source code and return the formatted result.
/// Returns None if no changes were made.
pub fn format_code(source: &str, language: &str, indent_size: usize) -> Option<String> {
    let indent_str: String = " ".repeat(indent_size);

    let result = match language {
        "python" => format_python(source, &indent_str),
        "html" | "htm" | "xml" | "svg" | "svelte" => format_html(source, &indent_str),
        _ => format_bracket_based(source, language, &indent_str),
    };

    // Only return if something actually changed
    if result == source {
        None
    } else {
        Some(result)
    }
}

// ==========================================================================
// Bracket-based formatter (C-like languages: JS, TS, Rust, CSS, JSON, etc.)
// ==========================================================================

fn format_bracket_based(source: &str, language: &str, indent: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = Vec::with_capacity(lines.len());
    let mut depth: i32 = 0;

    // State machine for string/comment tracking
    let mut in_block_comment = false;
    let mut in_string: Option<char> = None; // quote char
    let mut in_template_literal = false;
    let mut template_depth: i32 = 0; // track ${} nesting inside template literals

    for line in &lines {
        let trimmed = line.trim();

        // Skip empty lines — preserve them as-is
        if trimmed.is_empty() {
            result.push(String::new());
            continue;
        }

        // Handle block comment state
        if in_block_comment {
            // Indent block comment continuation at current depth
            let formatted = format!("{}{}", indent.repeat(depth.max(0) as usize), trimmed);
            result.push(formatted);
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }

        // Determine if this line starts with a closing bracket
        let first_char = trimmed.chars().next().unwrap_or(' ');
        let starts_with_close = matches!(first_char, '}' | ')' | ']');

        // For switch/case: dedent case/default labels one level
        let is_case_label = is_switch_case_label(trimmed, language);

        // Calculate indent for this line
        let line_depth = if starts_with_close {
            (depth - 1).max(0)
        } else if is_case_label {
            (depth - 1).max(0)
        } else {
            depth.max(0)
        };

        let formatted = format!("{}{}", indent.repeat(line_depth as usize), trimmed);
        result.push(formatted);

        // Update depth based on bracket analysis (respecting strings/comments)
        let (open, close, new_block_comment, new_in_string, new_template, new_template_depth) =
            count_brackets(trimmed, in_string, in_template_literal, template_depth, language);

        depth += open as i32 - close as i32;
        if depth < 0 {
            depth = 0;
        }
        in_block_comment = new_block_comment;
        in_string = new_in_string;
        in_template_literal = new_template;
        template_depth = new_template_depth;
    }

    // Preserve trailing newline if original had one
    let mut output = result.join("\n");
    if source.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Check if a line is a switch case/default label.
fn is_switch_case_label(trimmed: &str, language: &str) -> bool {
    match language {
        "javascript" | "typescript" | "jsx" | "tsx" | "java" | "c" | "cpp" | "csharp"
        | "kotlin" | "scala" | "dart" | "go" | "rust" | "swift" => {
            trimmed.starts_with("case ") || trimmed.starts_with("default:")
                || trimmed.starts_with("default ")
        }
        _ => false,
    }
}

/// Count opening and closing brackets on a line, respecting strings, comments, and regex literals.
/// Returns (opens, closes, entered_block_comment, string_state, template_state, template_depth).
fn count_brackets(
    line: &str,
    mut in_string: Option<char>,
    mut in_template: bool,
    mut tmpl_depth: i32,
    language: &str,
) -> (usize, usize, bool, Option<char>, bool, i32) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut in_block_comment = false;
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let supports_regex = matches!(language,
        "javascript" | "typescript" | "jsx" | "tsx" | "ruby" | "perl"
    );
    // Track last non-whitespace char to detect regex context
    let mut last_significant: char = ';'; // start-of-line acts like ';' (regex is valid)

    while i < len {
        let ch = chars[i];
        let next = if i + 1 < len { Some(chars[i + 1]) } else { None };

        // Inside a string literal
        if let Some(quote) = in_string {
            if ch == '\\' {
                i += 2; // skip escaped char
                continue;
            }
            if ch == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        // Inside a template literal (backtick)
        if in_template {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '$' && next == Some('{') {
                tmpl_depth += 1;
                opens += 1;
                i += 2;
                in_template = false; // inside ${}, normal JS rules apply
                last_significant = '{';
                continue;
            }
            if ch == '`' {
                in_template = false;
            }
            i += 1;
            continue;
        }

        // Skip whitespace (don't update last_significant)
        if ch.is_whitespace() {
            i += 1;
            continue;
        }

        // Line comment — skip rest of line
        if ch == '/' && next == Some('/') {
            break;
        }
        // Hash comment (Python, Ruby, Bash, YAML, TOML)
        if ch == '#' && matches!(language, "python" | "ruby" | "bash" | "yaml" | "toml" | "r" | "elixir" | "nix") {
            break;
        }
        // Lua/SQL line comment
        if ch == '-' && next == Some('-') && matches!(language, "lua" | "sql" | "haskell") {
            break;
        }

        // Block comment start
        if ch == '/' && next == Some('*') {
            // Check if it closes on the same line
            let rest = &line[i + 2..];
            if rest.contains("*/") {
                // Self-contained block comment on one line — skip
                i += 2;
                while i < len {
                    if chars[i] == '*' && i + 1 < len && chars[i + 1] == '/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            in_block_comment = true;
            break;
        }

        // Regex literal detection for JS/TS/Ruby:
        // A `/` starts a regex when preceded by an operator, keyword-end, or punctuation
        // (not after an identifier, number, or closing bracket)
        if ch == '/' && supports_regex && next != Some('/') && next != Some('*') {
            let is_regex_context = matches!(last_significant,
                '=' | '(' | ',' | '!' | '&' | '|' | '?' | ':' | ';' | '['
                | '{' | '}' | '+' | '-' | '*' | '%' | '<' | '>' | '~' | '^'
                | '\0' // start of analysis
            );
            if is_regex_context {
                // Skip the regex literal: consume until unescaped closing /
                i += 1; // skip opening /
                while i < len {
                    if chars[i] == '\\' {
                        i += 2; // skip escaped char inside regex
                        continue;
                    }
                    if chars[i] == '/' {
                        i += 1; // skip closing /
                        // Skip flags (g, i, m, s, u, y, d)
                        while i < len && chars[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        break;
                    }
                    i += 1;
                }
                last_significant = '/'; // regex acts like a value
                continue;
            }
        }

        // String literals
        if ch == '\'' || ch == '"' {
            in_string = Some(ch);
            last_significant = ch;
            i += 1;
            continue;
        }

        // Template literals (JS/TS)
        if ch == '`' && matches!(language, "javascript" | "typescript" | "jsx" | "tsx") {
            in_template = true;
            last_significant = '`';
            i += 1;
            continue;
        }

        // Closing template substitution
        if ch == '}' && tmpl_depth > 0 {
            tmpl_depth -= 1;
            closes += 1;
            in_template = true; // back to template literal content
            last_significant = '}';
            i += 1;
            continue;
        }

        // Count brackets
        match ch {
            '{' | '(' | '[' => opens += 1,
            '}' | ')' | ']' => closes += 1,
            _ => {}
        }

        last_significant = ch;
        i += 1;
    }

    (opens, closes, in_block_comment, in_string, in_template, tmpl_depth)
}

// ==========================================================================
// Python formatter
// ==========================================================================

/// Python formatting: we can't re-indent Python because indentation IS syntax.
/// Instead we: normalize tabs to spaces, trim trailing whitespace on each line,
/// and ensure consistent indent width (convert tab-based to space-based).
fn format_python(source: &str, indent: &str) -> String {
    let indent_size = indent.len();
    let lines: Vec<&str> = source.lines().collect();
    let mut result = Vec::with_capacity(lines.len());

    for line in &lines {
        if line.trim().is_empty() {
            result.push(String::new());
            continue;
        }

        // Count leading whitespace, converting tabs to indent_size spaces
        let mut leading_spaces = 0;
        let mut content_start = 0;
        for (i, ch) in line.char_indices() {
            match ch {
                ' ' => leading_spaces += 1,
                '\t' => leading_spaces += indent_size - (leading_spaces % indent_size),
                _ => { content_start = i; break; }
            }
            content_start = i + ch.len_utf8();
        }

        let content = line[content_start..].trim_end();
        if content.is_empty() {
            result.push(String::new());
        } else {
            result.push(format!("{}{}", " ".repeat(leading_spaces), content));
        }
    }

    let mut output = result.join("\n");
    if source.ends_with('\n') {
        output.push('\n');
    }
    output
}

// ==========================================================================
// HTML/XML formatter
// ==========================================================================

/// Tags that don't need closing and shouldn't increase indent depth.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input",
    "link", "meta", "param", "source", "track", "wbr",
    "!doctype", "!DOCTYPE",
];

/// Tags whose content should not be reformatted (preserve original whitespace).
const RAW_TAGS: &[&str] = &["script", "style", "pre", "code", "textarea"];

fn format_html(source: &str, indent: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = Vec::with_capacity(lines.len());
    let mut depth: i32 = 0;
    let mut in_raw_tag: Option<String> = None;

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            result.push(String::new());
            continue;
        }

        // Inside a raw tag (script/style/pre) — preserve original indentation
        if let Some(ref tag) = in_raw_tag {
            let close_pattern = format!("</{}>", tag);
            if trimmed.contains(&close_pattern) || trimmed.starts_with(&close_pattern) {
                in_raw_tag = None;
                // The closing tag itself gets formatted
                depth -= 1;
                let d = depth.max(0) as usize;
                result.push(format!("{}{}", indent.repeat(d), trimmed));
                continue;
            }
            // Preserve content inside raw tags but with base indent
            result.push(format!("{}{}", indent.repeat(depth.max(0) as usize), trimmed));
            continue;
        }

        // Analyze HTML tags on this line
        let (tag_opens, tag_closes, raw_enter) = analyze_html_line(trimmed);

        // If line starts with closing tag, dedent before printing this line
        let starts_with_close = trimmed.starts_with("</");
        if starts_with_close {
            depth -= 1;
            if depth < 0 { depth = 0; }
        }

        result.push(format!("{}{}", indent.repeat(depth.max(0) as usize), trimmed));

        // Update depth: add opens, subtract closes (but not the leading close we already handled)
        let remaining_closes = if starts_with_close { tag_closes.saturating_sub(1) } else { tag_closes };
        depth += tag_opens as i32 - remaining_closes as i32;
        if depth < 0 {
            depth = 0;
        }

        // Enter raw tag mode
        if let Some(tag) = raw_enter {
            in_raw_tag = Some(tag);
        }
    }

    let mut output = result.join("\n");
    if source.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Analyze an HTML line for opening/closing tags.
/// Returns (opens, closes, raw_tag_entered).
fn analyze_html_line(line: &str) -> (usize, usize, Option<String>) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut raw_enter: Option<String> = None;
    let mut in_string = false;
    let mut quote_char = ' ';

    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Handle attribute strings
        if in_string {
            if ch == quote_char {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = true;
            quote_char = ch;
            i += 1;
            continue;
        }

        // HTML comment <!-- ... -->
        if ch == '<' && i + 3 < len && chars[i + 1] == '!' && chars[i + 2] == '-' && chars[i + 3] == '-' {
            // Skip to end of comment
            i += 4;
            while i + 2 < len {
                if chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2] == '>' {
                    i += 3;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Closing tag </tag>
        if ch == '<' && i + 1 < len && chars[i + 1] == '/' {
            closes += 1;
            // Skip to >
            while i < len && chars[i] != '>' {
                i += 1;
            }
            i += 1;
            continue;
        }

        // Opening tag <tag...>
        if ch == '<' && i + 1 < len && chars[i + 1].is_alphabetic() || (ch == '<' && i + 1 < len && chars[i + 1] == '!') {
            // Extract tag name
            let tag_start = i + 1;
            let mut tag_end = tag_start;
            while tag_end < len && !chars[tag_end].is_whitespace() && chars[tag_end] != '>' && chars[tag_end] != '/' {
                tag_end += 1;
            }
            let tag_name: String = chars[tag_start..tag_end].iter().collect();
            let tag_lower = tag_name.to_lowercase();

            // Skip to end of tag
            let mut self_closing = false;
            while i < len {
                if chars[i] == '/' && i + 1 < len && chars[i + 1] == '>' {
                    self_closing = true;
                    i += 2;
                    break;
                }
                if chars[i] == '>' {
                    i += 1;
                    break;
                }
                i += 1;
            }

            let is_void = VOID_TAGS.iter().any(|v| v.to_lowercase() == tag_lower);

            if !self_closing && !is_void {
                opens += 1;
                // Check if this is a raw tag
                if RAW_TAGS.iter().any(|r| *r == tag_lower) {
                    raw_enter = Some(tag_lower);
                }
            }
            continue;
        }

        i += 1;
    }

    (opens, closes, raw_enter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bracket_based_js() {
        let input = "function foo() {\nlet x = 1;\nif (true) {\nx = 2;\n}\nreturn x;\n}\n";
        let expected = "function foo() {\n    let x = 1;\n    if (true) {\n        x = 2;\n    }\n    return x;\n}\n";
        assert_eq!(format_code(input, "javascript", 4).unwrap(), expected);
    }

    #[test]
    fn test_bracket_based_css() {
        let input = "body {\ncolor: red;\nbackground: blue;\n}\n";
        let expected = "body {\n    color: red;\n    background: blue;\n}\n";
        assert_eq!(format_code(input, "css", 4).unwrap(), expected);
    }

    #[test]
    fn test_python_tab_normalization() {
        // Python: we normalize tabs to spaces but don't re-indent
        let input = "def foo():\n\tx = 1\n\tif True:\n\t\tprint(x)\n\treturn x\n";
        let expected = "def foo():\n    x = 1\n    if True:\n        print(x)\n    return x\n";
        assert_eq!(format_code(input, "python", 4).unwrap(), expected);
    }

    #[test]
    fn test_python_trailing_whitespace() {
        let input = "def foo():   \n    x = 1   \n    return x  \n";
        let expected = "def foo():\n    x = 1\n    return x\n";
        assert_eq!(format_code(input, "python", 4).unwrap(), expected);
    }

    #[test]
    fn test_html_basic() {
        let input = "<html>\n<head>\n<title>Test</title>\n</head>\n<body>\n<p>Hello</p>\n</body>\n</html>\n";
        let expected = "<html>\n    <head>\n        <title>Test</title>\n    </head>\n    <body>\n        <p>Hello</p>\n    </body>\n</html>\n";
        assert_eq!(format_code(input, "html", 4).unwrap(), expected);
    }

    #[test]
    fn test_no_change() {
        let input = "function foo() {\n    return 1;\n}\n";
        assert_eq!(format_code(input, "javascript", 4), None);
    }

    #[test]
    fn test_strings_not_counted() {
        let input = "let x = \"{\";\nlet y = 1;\n";
        // Brackets inside strings should not affect indentation
        assert_eq!(format_code(input, "javascript", 4), None);
    }

    #[test]
    fn test_json() {
        let input = "{\n\"name\": \"test\",\n\"items\": [\n1,\n2\n]\n}\n";
        let expected = "{\n    \"name\": \"test\",\n    \"items\": [\n        1,\n        2\n    ]\n}\n";
        assert_eq!(format_code(input, "json", 4).unwrap(), expected);
    }

    #[test]
    fn test_js_regex_literals() {
        // Regex containing " and ' should NOT start a string
        let input = "function esc(str) {\nreturn String(str)\n.replace(/&/g, '&amp;')\n.replace(/</g, '&lt;')\n.replace(/>/g, '&gt;')\n.replace(/\"/g, '&quot;');\n}\n\nfunction next() {\nreturn 1;\n}\n";
        let result = format_code(input, "javascript", 4).unwrap();
        // After esc() closes, next() should be at depth 0
        assert!(result.contains("\nfunction next() {\n"), "next() should be at depth 0, got:\n{}", result);
        assert!(result.contains("\n    return 1;\n"), "return inside next() should be at depth 1");
    }

    #[test]
    fn test_js_regex_no_false_positive() {
        // Division operator should NOT be treated as regex — both sides of / should be preserved
        let input = "function foo() {\n    let x = a / b;\n    return x;\n}\n";
        // Already correctly formatted — no changes expected
        assert_eq!(format_code(input, "javascript", 4), None);
    }
}
