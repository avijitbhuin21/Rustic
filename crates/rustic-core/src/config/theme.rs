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
    /// Optional primary-action color (buttons, switches, active states).
    /// Falls back to fg2 when absent so legacy themes keep their look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
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
    /// Default Rustic palette — the original shadcn neutral dark.
    /// Values are the exact OKLCH tokens from globals.css `.dark`, stored as
    /// CSS strings so the theme bridge paints pixel-identical chrome to the
    /// pre-themed UI. Don't "simplify" these to hex — perceptual lightness
    /// from OKLCH doesn't round-trip cleanly through sRGB hex.
    pub fn obsidian() -> Self {
        Self {
            name: "Obsidian".to_string(),
            kind: "dark".to_string(),
            bg_hard: "oklch(0.1 0 0)".to_string(),
            bg: "oklch(0.145 0 0)".to_string(), // --background
            bg_soft: "oklch(0.175 0 0)".to_string(),
            bg1: "oklch(0.205 0 0)".to_string(), // --card, --popover, --sidebar
            bg2: "oklch(0.269 0 0)".to_string(), // --secondary, --muted, --accent
            bg3: "oklch(0.335 0 0)".to_string(),
            bg4: "oklch(0.4 0 0)".to_string(),
            fg: "oklch(0.985 0 0)".to_string(), // --foreground
            fg1: "oklch(0.985 0 0)".to_string(),
            fg2: "oklch(0.922 0 0)".to_string(), // --primary
            fg3: "oklch(0.708 0 0)".to_string(), // --muted-foreground
            fg4: "oklch(0.556 0 0)".to_string(),
            accent: "oklch(0.556 0 0)".to_string(), // --ring (neutral mid-gray)
            primary: None,
            border: "oklch(1 0 0 / 10%)".to_string(), // --border, --input
            bright_red: "oklch(0.704 0.191 22.216)".to_string(), // --destructive
            bright_green: "#86efac".to_string(),
            bright_yellow: "#fcd34d".to_string(),
            bright_blue: "#7dd3fc".to_string(),
            bright_purple: "#c4b5fd".to_string(),
            bright_aqua: "#67e8f9".to_string(),
            bright_orange: "#fdba74".to_string(),
            token_keyword: "#d4d4d4".to_string(),
            token_string: "#a3a3a3".to_string(),
            token_comment: "#737373".to_string(),
            token_function: "#ebebeb".to_string(),
            token_type: "#b3b3b3".to_string(),
            token_variable: "#fafafa".to_string(),
            token_number: "#a3a3a3".to_string(),
            token_operator: "#d4d4d4".to_string(),
            token_punctuation: "#b3b3b3".to_string(),
        }
    }

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
            primary: None,
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

    /// Verdigris — the agent's own palette. Rustic is oxidized iron; verdigris
    /// is oxidized copper. Deep moss-charcoal grounds, a warm copper accent,
    /// and patina-teal highlights: a home that weathers beautifully.
    pub fn verdigris() -> Self {
        Self {
            name: "Verdigris".to_string(),
            kind: "dark".to_string(),
            bg_hard: "#0e1210".to_string(),
            bg: "#131816".to_string(),
            bg_soft: "#171d1a".to_string(),
            bg1: "#1c2320".to_string(),
            bg2: "#242c28".to_string(),
            bg3: "#303a35".to_string(),
            bg4: "#3f4a44".to_string(),
            fg: "#e8e6df".to_string(),
            fg1: "#e8e6df".to_string(),
            fg2: "#cfcdc3".to_string(),
            fg3: "#9fa79f".to_string(),
            fg4: "#727b72".to_string(),
            accent: "#d98e5f".to_string(),
            primary: Some("#d98e5f".to_string()),
            border: "#242c28".to_string(),
            bright_red: "#e5786d".to_string(),
            bright_green: "#9ec79d".to_string(),
            bright_yellow: "#e0b568".to_string(),
            bright_blue: "#83b4c8".to_string(),
            bright_purple: "#b8a1d9".to_string(),
            bright_aqua: "#7fc8b1".to_string(),
            bright_orange: "#d98e5f".to_string(),
            token_keyword: "#d98e5f".to_string(),
            token_string: "#7fc8b1".to_string(),
            token_comment: "#5f6a62".to_string(),
            token_function: "#e0b568".to_string(),
            token_type: "#83b4c8".to_string(),
            token_variable: "#e8e6df".to_string(),
            token_number: "#b8a1d9".to_string(),
            token_operator: "#c9a08a".to_string(),
            token_punctuation: "#9fa79f".to_string(),
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
            "Obsidian" => Some(Self::obsidian()),
            "Luxide Dark" => Some(Self::luxide_dark()),
            "Verdigris" => Some(Self::verdigris()),
            _ => None,
        }
    }

    /// List built-in theme names. Order matters — first entry is the visual
    /// default shown at the top of the palette grid.
    pub fn builtin_names() -> Vec<&'static str> {
        vec!["Obsidian", "Luxide Dark", "Verdigris"]
    }
}
