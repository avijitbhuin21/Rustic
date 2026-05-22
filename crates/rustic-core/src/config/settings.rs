use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub general: GeneralSettings,
    #[serde(default)]
    pub editor: EditorSettings,
    #[serde(default)]
    pub theme: ThemeSettings,
    #[serde(default)]
    pub keybindings: Vec<super::keymap::Keybinding>,
    #[serde(default)]
    pub ai: AiSettings,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            general: GeneralSettings::default(),
            editor: EditorSettings::default(),
            theme: ThemeSettings::default(),
            keybindings: Vec::new(),
            ai: AiSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub font_family: String,
    pub font_size: f32,
    pub ui_scale: f32,
    pub auto_save: bool,
    pub auto_save_delay_ms: u64,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono, Fira Code, Consolas, monospace".to_string(),
            font_size: 14.0,
            ui_scale: 1.0,
            auto_save: false,
            auto_save_delay_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    // Tab & Indentation
    pub tab_size: u32,
    pub insert_spaces: bool,
    #[serde(default = "default_auto_indent")]
    pub auto_indent: String,

    // Display
    pub word_wrap: bool,
    pub line_numbers: bool,
    pub minimap: bool,
    pub render_whitespace: String,
    #[serde(default)]
    pub show_zero_width_characters: bool,
    #[serde(default = "default_true")]
    pub bracket_pair_colorization: bool,
    #[serde(default = "default_true")]
    pub format_on_save: bool,
    #[serde(default = "default_true")]
    pub sticky_scroll: bool,
    #[serde(default = "default_true")]
    pub smooth_scrolling: bool,
    #[serde(default = "default_true")]
    pub indent_guides: bool,

    // Cursor
    pub cursor_blink: bool,
    pub cursor_style: String,
    #[serde(default = "default_cursor_caret")]
    pub cursor_smooth_caret: String,

    // Font (kept here so Monaco settings live together)
    #[serde(default)]
    pub font_family: String,
}

fn default_true() -> bool { true }
fn default_auto_indent() -> String { "advanced".to_string() }
fn default_cursor_caret() -> String { "off".to_string() }

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            auto_indent: default_auto_indent(),
            word_wrap: false,
            line_numbers: true,
            minimap: false,
            render_whitespace: "none".to_string(),
            show_zero_width_characters: false,
            bracket_pair_colorization: true,
            format_on_save: true,
            sticky_scroll: true,
            smooth_scrolling: true,
            indent_guides: true,
            cursor_blink: true,
            cursor_style: "line".to_string(),
            cursor_smooth_caret: default_cursor_caret(),
            font_family: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeSettings {
    pub active_theme: String,
    pub custom_themes: Vec<String>,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            active_theme: "Obsidian".to_string(),
            custom_themes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    pub default_provider: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            default_provider: None,
            max_tokens: 4096,
            temperature: 0.7,
        }
    }
}

