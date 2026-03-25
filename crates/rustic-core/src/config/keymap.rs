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
    pub fn from_vscode_json(json: &str) -> Result<Self, String> {
        let bindings: Vec<Keybinding> =
            serde_json::from_str(json).map_err(|e| format!("Invalid keybindings JSON: {}", e))?;
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
            ],
        }
    }
}
