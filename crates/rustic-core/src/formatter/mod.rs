/// Languages that have a real formatting implementation. Any language absent
/// from this set is skipped silently so unsupported file types (`.env`,
/// `.txt`, unknown extensions, etc.) are never touched.
const SUPPORTED_LANGUAGES: &[&str] = &[
    "javascript",
    "typescript",
    "jsx",
    "tsx",
    "rust",
    "go",
    "c",
    "cpp",
    "java",
    "kotlin",
    "scala",
    "swift",
    "dart",
    "csharp",
    "css",
    "scss",
    "json",
    "python",
    "html",
    "htm",
    "xml",
    "svg",
    "svelte",
    "lua",
    "ruby",
    "php",
    "r",
    "zig",
    "elixir",
    "haskell",
    "nix",
];

/// Format source code and return the formatted result.
/// Returns `None` when the language has no formatter support, or when the
/// content is already correctly formatted (no changes needed).
pub fn format_code(source: &str, language: &str, indent_size: usize, use_tabs: bool) -> Option<String> {
    if !SUPPORTED_LANGUAGES.contains(&language) {
        return None;
    }

    let indent_str: String = if use_tabs {
        "\t".to_string()
    } else {
        " ".repeat(indent_size)
    };

    let result = match language {
        "python" => format_python(source, &indent_str, use_tabs, indent_size),
        "html" | "htm" | "xml" | "svg" | "svelte" => format_html(source, &indent_str),
        _ => format_bracket_based(source, language, &indent_str),
    };

    if result == source {
        None
    } else {
        Some(result)
    }
}

fn format_bracket_based(source: &str, language: &str, indent: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = Vec::with_capacity(lines.len());
    let mut depth: i32 = 0;

    let mut in_block_comment = false;
    let mut in_string: Option<char> = None;
    let mut in_template_literal = false;
    let mut template_depth: i32 = 0;

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            result.push(String::new());
            continue;
        }

        if in_block_comment {
            let formatted = format!("{}{}", indent.repeat(depth.max(0) as usize), trimmed);
            result.push(formatted);
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }

        let first_char = trimmed.chars().next().unwrap_or(' ');
        let starts_with_close = matches!(first_char, '}' | ')' | ']');
        let is_case_label = is_switch_case_label(trimmed, language);

        let line_depth = if starts_with_close {
            (depth - 1).max(0)
        } else if is_case_label {
            (depth - 1).max(0)
        } else {
            depth.max(0)
        };

        let formatted = format!("{}{}", indent.repeat(line_depth as usize), trimmed);
        result.push(formatted);

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
    // ';' at start-of-line means regex is valid here.
    let mut last_significant: char = ';';

    while i < len {
        let ch = chars[i];
        let next = if i + 1 < len { Some(chars[i + 1]) } else { None };

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

        if ch.is_whitespace() {
            i += 1;
            continue;
        }

        if ch == '/' && next == Some('/') {
            break;
        }
        if ch == '#' && matches!(language, "python" | "ruby" | "bash" | "yaml" | "toml" | "r" | "elixir" | "nix") {
            break;
        }
        if ch == '-' && next == Some('-') && matches!(language, "lua" | "sql" | "haskell") {
            break;
        }

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

        if ch == '/' && supports_regex && next != Some('/') && next != Some('*') {
            let is_regex_context = matches!(last_significant,
                '=' | '(' | ',' | '!' | '&' | '|' | '?' | ':' | ';' | '['
                | '{' | '}' | '+' | '-' | '*' | '%' | '<' | '>' | '~' | '^'
                | '\0' // start of analysis
            );
            if is_regex_context {
                i += 1;
                while i < len {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if chars[i] == '/' {
                        i += 1;
                        while i < len && chars[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        break;
                    }
                    i += 1;
                }
                last_significant = '/';
                continue;
            }
        }

        if ch == '\'' || ch == '"' {
            in_string = Some(ch);
            last_significant = ch;
            i += 1;
            continue;
        }

        if ch == '`' && matches!(language, "javascript" | "typescript" | "jsx" | "tsx") {
            in_template = true;
            last_significant = '`';
            i += 1;
            continue;
        }

        if ch == '}' && tmpl_depth > 0 {
            tmpl_depth -= 1;
            closes += 1;
            in_template = true;
            last_significant = '}';
            i += 1;
            continue;
        }

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

/// Python indentation IS syntax — we only normalize leading whitespace style and trim trailing spaces.
fn format_python(source: &str, indent: &str, use_tabs: bool, indent_size: usize) -> String {
    let tab_width = if indent_size == 0 { 4 } else { indent_size };
    let lines: Vec<&str> = source.lines().collect();
    let mut result = Vec::with_capacity(lines.len());

    for line in &lines {
        if line.trim().is_empty() {
            result.push(String::new());
            continue;
        }

        let mut leading_spaces = 0usize;
        let mut content_start = 0usize;
        for (i, ch) in line.char_indices() {
            match ch {
                ' ' => leading_spaces += 1,
                '\t' => leading_spaces += tab_width - (leading_spaces % tab_width),
                _ => { content_start = i; break; }
            }
            content_start = i + ch.len_utf8();
        }

        let content = line[content_start..].trim_end();
        if content.is_empty() {
            result.push(String::new());
        } else {
            let level = if tab_width == 0 { 0 } else { leading_spaces / tab_width };
            let remainder = leading_spaces % tab_width;
            let prefix = if use_tabs {
                format!("{}{}", indent.repeat(level), " ".repeat(remainder))
            } else {
                " ".repeat(leading_spaces)
            };
            result.push(format!("{}{}", prefix, content));
        }
    }

    let mut output = result.join("\n");
    if source.ends_with('\n') {
        output.push('\n');
    }
    output
}

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

        if let Some(ref tag) = in_raw_tag {
            let close_pattern = format!("</{}>", tag);
            if trimmed.contains(&close_pattern) || trimmed.starts_with(&close_pattern) {
                in_raw_tag = None;
                depth -= 1;
                let d = depth.max(0) as usize;
                result.push(format!("{}{}", indent.repeat(d), trimmed));
                continue;
            }
            // Preserve content inside raw tags but with base indent
            result.push(format!("{}{}", indent.repeat(depth.max(0) as usize), trimmed));
            continue;
        }

        let (tag_opens, tag_closes, raw_enter) = analyze_html_line(trimmed);

        let starts_with_close = trimmed.starts_with("</");
        if starts_with_close {
            depth -= 1;
            if depth < 0 { depth = 0; }
        }

        result.push(format!("{}{}", indent.repeat(depth.max(0) as usize), trimmed));

        let remaining_closes = if starts_with_close { tag_closes.saturating_sub(1) } else { tag_closes };
        depth += tag_opens as i32 - remaining_closes as i32;
        if depth < 0 {
            depth = 0;
        }

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

        if ch == '<' && i + 3 < len && chars[i + 1] == '!' && chars[i + 2] == '-' && chars[i + 3] == '-' {
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

        if ch == '<' && i + 1 < len && chars[i + 1] == '/' {
            closes += 1;
            while i < len && chars[i] != '>' {
                i += 1;
            }
            i += 1;
            continue;
        }

        if ch == '<' && i + 1 < len && chars[i + 1].is_alphabetic() || (ch == '<' && i + 1 < len && chars[i + 1] == '!') {
            let tag_start = i + 1;
            let mut tag_end = tag_start;
            while tag_end < len && !chars[tag_end].is_whitespace() && chars[tag_end] != '>' && chars[tag_end] != '/' {
                tag_end += 1;
            }
            let tag_name: String = chars[tag_start..tag_end].iter().collect();
            let tag_lower = tag_name.to_lowercase();

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
        assert_eq!(format_code(input, "javascript", 4, false).unwrap(), expected);
    }

    #[test]
    fn test_bracket_based_css() {
        let input = "body {\ncolor: red;\nbackground: blue;\n}\n";
        let expected = "body {\n    color: red;\n    background: blue;\n}\n";
        assert_eq!(format_code(input, "css", 4, false).unwrap(), expected);
    }

    #[test]
    fn test_python_tab_normalization() {
        let input = "def foo():\n\tx = 1\n\tif True:\n\t\tprint(x)\n\treturn x\n";
        let expected = "def foo():\n    x = 1\n    if True:\n        print(x)\n    return x\n";
        assert_eq!(format_code(input, "python", 4, false).unwrap(), expected);
    }

    #[test]
    fn test_python_trailing_whitespace() {
        let input = "def foo():   \n    x = 1   \n    return x  \n";
        let expected = "def foo():\n    x = 1\n    return x\n";
        assert_eq!(format_code(input, "python", 4, false).unwrap(), expected);
    }

    #[test]
    fn test_html_basic() {
        let input = "<html>\n<head>\n<title>Test</title>\n</head>\n<body>\n<p>Hello</p>\n</body>\n</html>\n";
        let expected = "<html>\n    <head>\n        <title>Test</title>\n    </head>\n    <body>\n        <p>Hello</p>\n    </body>\n</html>\n";
        assert_eq!(format_code(input, "html", 4, false).unwrap(), expected);
    }

    #[test]
    fn test_no_change() {
        let input = "function foo() {\n    return 1;\n}\n";
        assert_eq!(format_code(input, "javascript", 4, false), None);
    }

    #[test]
    fn test_strings_not_counted() {
        let input = "let x = \"{\";\nlet y = 1;\n";
        assert_eq!(format_code(input, "javascript", 4, false), None);
    }

    #[test]
    fn test_json() {
        let input = "{\n\"name\": \"test\",\n\"items\": [\n1,\n2\n]\n}\n";
        let expected = "{\n    \"name\": \"test\",\n    \"items\": [\n        1,\n        2\n    ]\n}\n";
        assert_eq!(format_code(input, "json", 4, false).unwrap(), expected);
    }

    #[test]
    fn test_js_regex_literals() {
        let input = "function esc(str) {\nreturn String(str)\n.replace(/&/g, '&amp;')\n.replace(/</g, '&lt;')\n.replace(/>/g, '&gt;')\n.replace(/\"/g, '&quot;');\n}\n\nfunction next() {\nreturn 1;\n}\n";
        let result = format_code(input, "javascript", 4, false).unwrap();
        assert!(result.contains("\nfunction next() {\n"), "next() should be at depth 0, got:\n{}", result);
        assert!(result.contains("\n    return 1;\n"), "return inside next() should be at depth 1");
    }

    #[test]
    fn test_js_regex_no_false_positive() {
        let input = "function foo() {\n    let x = a / b;\n    return x;\n}\n";
        assert_eq!(format_code(input, "javascript", 4, false), None);
    }

    #[test]
    fn test_unsupported_language_skipped() {
        assert_eq!(format_code("KEY=value\n", "toml", 4, false), None);
        assert_eq!(format_code("hello world\n", "text", 4, false), None);
        assert_eq!(format_code("KEY=value\n", "unknown", 4, false), None);
        assert_eq!(format_code("NAME=Alice\n", "bash", 4, false), None);
        assert_eq!(format_code("# comment\n", "yaml", 4, false), None);
        assert_eq!(format_code("# comment\n", "markdown", 4, false), None);
        assert_eq!(format_code("SELECT 1;\n", "sql", 4, false), None);
    }

    #[test]
    fn test_tab_indent_preserved() {
        let input = "function foo() {\nlet x = 1;\n}\n";
        let result = format_code(input, "javascript", 4, true).unwrap();
        assert!(result.contains('\t'), "tab-indented output should contain tabs");
        assert!(result.contains("\tlet x = 1;"), "body should be tab-indented");
    }
}
