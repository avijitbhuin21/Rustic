use serde::{Deserialize, Serialize};

/// A single keybinding entry, compatible with VS Code's keybindings.json format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keybinding {
    pub key: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// A set of keybindings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingSet {
    pub bindings: Vec<Keybinding>,
}

impl KeybindingSet {
    /// Import from a VS Code-compatible keybindings.json string.
    /// VS Code uses JSONC (JSON with comments and trailing commas), so
    /// strip those before strict-parsing. Without this, every real-world
    /// keybindings.json fails to parse because VS Code seeds it with a
    /// `// Place your key bindings in this file…` header comment.
    pub fn from_vscode_json(json: &str) -> Result<Self, String> {
        let cleaned = strip_jsonc(json);
        let bindings: Vec<Keybinding> = serde_json::from_str(&cleaned)
            .map_err(|e| format!("Invalid keybindings JSON: {}", e))?;
        Ok(Self { bindings })
    }

    /// Default keybindings matching common VS Code shortcuts.
    pub fn defaults() -> Self {
        Self {
            bindings: vec![
                Keybinding { key: "ctrl+s".into(), command: "file.save".into(), when: None },
                Keybinding { key: "ctrl+z".into(), command: "edit.undo".into(), when: None },
                Keybinding { key: "ctrl+shift+z".into(), command: "edit.redo".into(), when: None },
                Keybinding { key: "ctrl+y".into(), command: "edit.redo".into(), when: None },
                Keybinding { key: "ctrl+w".into(), command: "tab.close".into(), when: None },
                Keybinding { key: "ctrl+tab".into(), command: "tab.next".into(), when: None },
                Keybinding { key: "ctrl+shift+tab".into(), command: "tab.previous".into(), when: None },
                Keybinding { key: "ctrl+p".into(), command: "quickOpen.show".into(), when: None },
                Keybinding { key: "ctrl+shift+p".into(), command: "commandPalette.show".into(), when: None },
                Keybinding { key: "ctrl+shift+f".into(), command: "search.show".into(), when: None },
                Keybinding { key: "ctrl+b".into(), command: "sidebar.toggle".into(), when: None },
                Keybinding { key: "ctrl+j".into(), command: "panel.toggle".into(), when: None },
                Keybinding { key: "ctrl+`".into(), command: "terminal.new".into(), when: None },
                Keybinding { key: "ctrl+n".into(), command: "file.new".into(), when: None },
                Keybinding { key: "ctrl+shift+n".into(), command: "window.new".into(), when: None },
                Keybinding { key: "ctrl+,".into(), command: "settings.show".into(), when: None },
                Keybinding { key: "alt+shift+f".into(), command: "editor.formatDocument".into(), when: None },
            ],
        }
    }
}

/// Strip JSONC comments (`// …` and `/* … */`) and trailing commas from a
/// JSON string so strict serde_json can parse it. Comments inside string
/// literals are preserved.
fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c as char);
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = true;
            out.push('"');
            i += 1;
            continue;
        }
        // Line comment
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    // Strip trailing commas: ,] and ,} (with optional whitespace between).
    let mut cleaned = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    let mut j = 0;
    while j < chars.len() {
        if chars[j] == ',' {
            let mut k = j + 1;
            while k < chars.len() && chars[k].is_whitespace() {
                k += 1;
            }
            if k < chars.len() && (chars[k] == ']' || chars[k] == '}') {
                // skip the comma
                j += 1;
                continue;
            }
        }
        cleaned.push(chars[j]);
        j += 1;
    }
    cleaned
}
