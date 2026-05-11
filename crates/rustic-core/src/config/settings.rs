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
    pub tab_size: u32,
    pub insert_spaces: bool,
    pub word_wrap: bool,
    pub line_numbers: bool,
    pub minimap: bool,
    pub cursor_blink: bool,
    pub cursor_style: String,
    pub render_whitespace: String,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            word_wrap: false,
            line_numbers: true,
            minimap: false,
            cursor_blink: true,
            cursor_style: "line".to_string(),
            render_whitespace: "none".to_string(),
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
            active_theme: "Luxide Dark".to_string(),
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

