use serde::{Deserialize, Serialize};

/// Complete theme definition with all color slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String, // "dark" or "light"

    // Backgrounds
    pub bg_hard: String,
    pub bg: String,
    pub bg_soft: String,
    pub bg1: String,
    pub bg2: String,
    pub bg3: String,
    pub bg4: String,

    // Foregrounds
    pub fg: String,
    pub fg1: String,
    pub fg2: String,
    pub fg3: String,
    pub fg4: String,

    // Accent
    pub accent: String,
    pub border: String,

    // Bright colors
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_purple: String,
    pub bright_aqua: String,
    pub bright_orange: String,

    // Token colors (syntax highlighting)
    pub token_keyword: String,
    pub token_string: String,
    pub token_comment: String,
    pub token_function: String,
    pub token_type: String,
    pub token_variable: String,
    pub token_number: String,
    pub token_operator: String,
    pub token_punctuation: String,
}

fn default_kind() -> String {
    "dark".to_string()
}

impl Theme {
    pub fn luxide_dark() -> Self {
        Self {
            name: "Luxide Dark".to_string(),
            kind: "dark".to_string(),
            bg_hard: "#0d0e12".to_string(),
            bg: "#13141a".to_string(),
            bg_soft: "#181a21".to_string(),
            bg1: "#1e2028".to_string(),
            bg2: "#272932".to_string(),
            bg3: "#33363f".to_string(),
            bg4: "#43464f".to_string(),
            fg: "#e4e4ec".to_string(),
            fg1: "#e4e4ec".to_string(),
            fg2: "#c8c8d4".to_string(),
            fg3: "#9a9ab0".to_string(),
            fg4: "#71718a".to_string(),
            accent: "#a78bfa".to_string(),
            border: "#272932".to_string(),
            bright_red: "#f87171".to_string(),
            bright_green: "#86efac".to_string(),
            bright_yellow: "#fcd34d".to_string(),
            bright_blue: "#7dd3fc".to_string(),
            bright_purple: "#c4b5fd".to_string(),
            bright_aqua: "#67e8f9".to_string(),
            bright_orange: "#fdba74".to_string(),
            token_keyword: "#c4b5fd".to_string(),
            token_string: "#86efac".to_string(),
            token_comment: "#5a5a72".to_string(),
            token_function: "#fcd34d".to_string(),
            token_type: "#7dd3fc".to_string(),
            token_variable: "#e4e4ec".to_string(),
            token_number: "#fdba74".to_string(),
            token_operator: "#a78bfa".to_string(),
            token_punctuation: "#9a9ab0".to_string(),
        }
    }

    /// Parse a theme from TOML content.
    pub fn from_toml(content: &str) -> Result<Self, String> {
        toml::from_str(content).map_err(|e| format!("Invalid TOML theme: {}", e))
    }

    /// Parse a theme from JSON content.
    pub fn from_json(content: &str) -> Result<Self, String> {
        serde_json::from_str(content).map_err(|e| format!("Invalid JSON theme: {}", e))
    }

    /// Get a built-in theme by name.
    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "Luxide Dark" => Some(Self::luxide_dark()),
            _ => None,
        }
    }

    /// List built-in theme names.
    pub fn builtin_names() -> Vec<&'static str> {
        vec!["Luxide Dark"]
    }
}
