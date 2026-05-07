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
    pub fn gruvbox_dark() -> Self {
        Self {
            name: "Gruvbox Dark".to_string(),
            kind: "dark".to_string(),
            bg_hard: "#1d2021".to_string(),
            bg: "#282828".to_string(),
            bg_soft: "#32302f".to_string(),
            bg1: "#3c3836".to_string(),
            bg2: "#504945".to_string(),
            bg3: "#665c54".to_string(),
            bg4: "#7c6f64".to_string(),
            fg: "#ebdbb2".to_string(),
            fg1: "#ebdbb2".to_string(),
            fg2: "#d5c4a1".to_string(),
            fg3: "#bdae93".to_string(),
            fg4: "#a89984".to_string(),
            accent: "#fe8019".to_string(),
            border: "#3c3836".to_string(),
            bright_red: "#fb4934".to_string(),
            bright_green: "#b8bb26".to_string(),
            bright_yellow: "#fabd2f".to_string(),
            bright_blue: "#83a598".to_string(),
            bright_purple: "#d3869b".to_string(),
            bright_aqua: "#8ec07c".to_string(),
            bright_orange: "#fe8019".to_string(),
            token_keyword: "#fb4934".to_string(),
            token_string: "#b8bb26".to_string(),
            token_comment: "#928374".to_string(),
            token_function: "#fabd2f".to_string(),
            token_type: "#83a598".to_string(),
            token_variable: "#ebdbb2".to_string(),
            token_number: "#d3869b".to_string(),
            token_operator: "#fe8019".to_string(),
            token_punctuation: "#a89984".to_string(),
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
            "Gruvbox Dark" => Some(Self::gruvbox_dark()),
            _ => None,
        }
    }

    /// List built-in theme names.
    pub fn builtin_names() -> Vec<&'static str> {
        vec!["Gruvbox Dark"]
    }
}
