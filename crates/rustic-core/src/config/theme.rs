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

    /// Build a theme from its 31 color slots in field order:
    /// bg_hard bg bg_soft bg1 bg2 bg3 bg4 · fg fg1 fg2 fg3 fg4 · accent primary
    /// border · red green yellow blue purple aqua orange · keyword string
    /// comment function type variable number operator punctuation.
    fn palette(name: &str, kind: &str, c: [&str; 31]) -> Self {
        let s = |i: usize| c[i].to_string();
        Self {
            name: name.to_string(),
            kind: kind.to_string(),
            bg_hard: s(0), bg: s(1), bg_soft: s(2), bg1: s(3), bg2: s(4), bg3: s(5), bg4: s(6),
            fg: s(7), fg1: s(8), fg2: s(9), fg3: s(10), fg4: s(11),
            accent: s(12), primary: Some(s(13)), border: s(14),
            bright_red: s(15), bright_green: s(16), bright_yellow: s(17), bright_blue: s(18),
            bright_purple: s(19), bright_aqua: s(20), bright_orange: s(21),
            token_keyword: s(22), token_string: s(23), token_comment: s(24), token_function: s(25),
            token_type: s(26), token_variable: s(27), token_number: s(28), token_operator: s(29),
            token_punctuation: s(30),
        }
    }

    // Palette families (light + dark), contrast-checked: text ≥ 14:1, muted
    // text ≥ 5.5:1, button text on primary ≥ 4.5:1, status + syntax ≥ 4.5:1.

    /// Graphite — cool neutral grey, cobalt accent.
    pub fn graphite() -> Self {
        Self::palette("Graphite", "dark", [
            "#0b0d10", "#111317", "#15181d", "#1a1d23", "#23272e", "#2e333b", "#3b414a",
            "#e6e8eb", "#e6e8eb", "#c9cdd3", "#959ba5", "#6c727c",
            "#6b9bff", "#6b9bff", "#252a31",
            "#f2777a", "#7fcf9a", "#e8c46a", "#6b9bff", "#b49cf0", "#6cc9d4", "#ee9a62",
            "#8fb2ff", "#93d3a8", "#6f7680", "#e8c46a", "#6cc9d4", "#e6e8eb", "#ee9a62", "#a9b0ba", "#8a919b",
        ])
    }

    /// Graphite Light — the same family on near-white.
    pub fn graphite_light() -> Self {
        Self::palette("Graphite Light", "light", [
            "#e7e9ec", "#f6f7f9", "#f0f2f5", "#ffffff", "#eceef2", "#dfe2e7", "#cbd0d7",
            "#16191d", "#16191d", "#2c3138", "#5b626c", "#8a919b",
            "#2f5fd0", "#2f5fd0", "#dde1e6",
            "#c4373a", "#1f7a45", "#9a6b00", "#2f5fd0", "#6b4fc4", "#0f7c86", "#b45a1c",
            "#2f5fd0", "#1f7a45", "#6b727c", "#8a5a00", "#0f7c86", "#16191d", "#b45a1c", "#4a515b", "#5b626c",
        ])
    }

    /// Ink — warm near-black with a brass accent (night editorial).
    pub fn ink() -> Self {
        Self::palette("Ink", "dark", [
            "#0f0d0b", "#151310", "#1a1714", "#201c18", "#2a2520", "#36302a", "#463e36",
            "#ece4d8", "#ece4d8", "#d6ccbd", "#a39886", "#786e60",
            "#d9b26f", "#d9b26f", "#2c2721",
            "#e07a68", "#a8c48a", "#d9b26f", "#8fb0c9", "#c1a0c8", "#8cc2b4", "#df9b62",
            "#df9b62", "#a8c48a", "#7d7366", "#d9b26f", "#8fb0c9", "#ece4d8", "#c1a0c8", "#b8ab97", "#a39886",
        ])
    }

    /// Paper — warm parchment with an ink-blue accent.
    pub fn paper() -> Self {
        Self::palette("Paper", "light", [
            "#ebe5d9", "#f7f3ea", "#f1ecdf", "#fdfbf6", "#ece6d8", "#e0d8c6", "#cbbfa8",
            "#221d17", "#221d17", "#3a3229", "#6b6052", "#968a79",
            "#2d4f7c", "#2d4f7c", "#e2dacb",
            "#b23a2c", "#4a6b2a", "#8a6510", "#2d4f7c", "#6d4a7e", "#2c6e66", "#a5521f",
            "#a5521f", "#4a6b2a", "#7d7264", "#2d4f7c", "#2c6e66", "#221d17", "#6d4a7e", "#5a5044", "#6b6052",
        ])
    }

    /// Fjord Night — cool slate with a glacier-teal accent.
    pub fn fjord_night() -> Self {
        Self::palette("Fjord Night", "dark", [
            "#0a0f14", "#0f151c", "#131a22", "#172029", "#1f2a35", "#2a3744", "#384756",
            "#e2e9ef", "#e2e9ef", "#c3cdd6", "#8d9aa8", "#66727f",
            "#5cc6c1", "#5cc6c1", "#213040",
            "#ef7d7d", "#7fd1a4", "#e6c770", "#78aee8", "#a99be8", "#5cc6c1", "#ec9f6d",
            "#78aee8", "#7fd1a4", "#66737f", "#5cc6c1", "#e6c770", "#e2e9ef", "#ec9f6d", "#a3b1be", "#8d9aa8",
        ])
    }

    /// Fjord Day — the same family in daylight.
    pub fn fjord_day() -> Self {
        Self::palette("Fjord Day", "light", [
            "#e3e9ee", "#f3f6f8", "#ecf1f4", "#ffffff", "#e6edf2", "#d6e0e7", "#bfccd6",
            "#121a22", "#121a22", "#26323d", "#52606d", "#84919d",
            "#0e7c78", "#0e7c78", "#d8e1e8",
            "#c03b3b", "#1d7a4e", "#8f6a00", "#1f63b5", "#5a4bb8", "#0e7c78", "#b35a1e",
            "#1f63b5", "#1d7a4e", "#6a7783", "#0e7c78", "#8f6a00", "#121a22", "#b35a1e", "#3d4a56", "#52606d",
        ])
    }

    /// Kiln — stone neutrals with a terracotta accent.
    pub fn kiln() -> Self {
        Self::palette("Kiln", "dark", [
            "#0e0c0b", "#141210", "#191614", "#1f1b18", "#292420", "#352f2a", "#453d37",
            "#ebe6e1", "#ebe6e1", "#d2cbc4", "#9f968d", "#756c64",
            "#e0835a", "#e0835a", "#2b2622",
            "#e86f62", "#9fc48e", "#e3b964", "#86aed0", "#c09fd1", "#7fc1b5", "#e0835a",
            "#e0835a", "#9fc48e", "#7a7068", "#e3b964", "#86aed0", "#ebe6e1", "#c09fd1", "#bfb3a8", "#9f968d",
        ])
    }

    /// Kiln Light — warm stone daylight, fired-clay accent.
    pub fn kiln_light() -> Self {
        Self::palette("Kiln Light", "light", [
            "#ebe6e1", "#f8f5f2", "#f2eee9", "#fffdfb", "#eee8e2", "#e1d9d1", "#cbc0b5",
            "#1f1a16", "#1f1a16", "#352e28", "#665b51", "#93887e",
            "#b0532c", "#b0532c", "#e3dbd3",
            "#b8352a", "#3f6e33", "#8a6408", "#2f5f8f", "#6a4a8c", "#24706a", "#b0532c",
            "#b0532c", "#3f6e33", "#7c7168", "#8a6408", "#2f5f8f", "#1f1a16", "#6a4a8c", "#574c43", "#665b51",
        ])
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
            "Graphite" => Some(Self::graphite()),
            "Graphite Light" => Some(Self::graphite_light()),
            "Ink" => Some(Self::ink()),
            "Paper" => Some(Self::paper()),
            "Fjord Night" => Some(Self::fjord_night()),
            "Fjord Day" => Some(Self::fjord_day()),
            "Kiln" => Some(Self::kiln()),
            "Kiln Light" => Some(Self::kiln_light()),
            _ => None,
        }
    }

    /// List built-in theme names. Order matters — first entry is the visual
    /// default shown at the top of the palette grid; families sit light-after-dark.
    pub fn builtin_names() -> Vec<&'static str> {
        vec![
            "Obsidian", "Luxide Dark", "Verdigris",
            "Graphite", "Graphite Light", "Ink", "Paper",
            "Fjord Night", "Fjord Day", "Kiln", "Kiln Light",
        ]
    }
}
