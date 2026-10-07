//! Theme packs (format 2): one file per theme under `<data>/themes/`, shared by
//! the desktop app and rustic-server. A pack carries metadata, a dark and/or
//! light color set (missing slots fall back to a built-in base), style
//! settings, and optional custom CSS.
//!
//! Safety model (user decision): imported / edited themes never apply until
//! the user trusts that exact file (sha-256 of its bytes). Every pack is
//! scanned: script-like CSS is blocked outright, risky-but-legit patterns
//! (remote `@import` / `url()`, full-screen overlays, hiding rules) are listed
//! as warnings in the trust prompt. Custom CSS is wrapped in
//! `@scope (:root) to ([data-rustic-protected])` so it can't restyle the trust
//! prompt, permission prompts or pairing / transfer approvals.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rustic_core::config::Theme;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state::AppState;
use crate::sync_ext::MutexExt;

/// Current pack format.
pub const FORMAT: u32 = 2;
/// Theme file extension.
pub const EXT: &str = ".rustic-theme.json";
/// Largest accepted theme file.
pub const MAX_FILE_BYTES: usize = 512 * 1024;
/// DB key holding `{ theme_id: sha256 }` of trusted theme files.
const TRUST_KEY: &str = "theme_trust";

/// The 31 color slots of a mode; any may be omitted (falls back to the base).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Colors {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg_hard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg_soft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bg4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub fg1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub fg2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub fg3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub fg4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub primary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_red: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_green: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_yellow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_blue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_purple: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_aqua: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bright_orange: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_variable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub token_punctuation: Option<String>,
}

/// `(name, &value)` for every slot, in field order.
macro_rules! color_fields {
    ($c:expr) => {
        [
            ("bg_hard", &$c.bg_hard), ("bg", &$c.bg), ("bg_soft", &$c.bg_soft), ("bg1", &$c.bg1),
            ("bg2", &$c.bg2), ("bg3", &$c.bg3), ("bg4", &$c.bg4), ("fg", &$c.fg), ("fg1", &$c.fg1),
            ("fg2", &$c.fg2), ("fg3", &$c.fg3), ("fg4", &$c.fg4), ("accent", &$c.accent),
            ("primary", &$c.primary), ("border", &$c.border), ("bright_red", &$c.bright_red),
            ("bright_green", &$c.bright_green), ("bright_yellow", &$c.bright_yellow),
            ("bright_blue", &$c.bright_blue), ("bright_purple", &$c.bright_purple),
            ("bright_aqua", &$c.bright_aqua), ("bright_orange", &$c.bright_orange),
            ("token_keyword", &$c.token_keyword), ("token_string", &$c.token_string),
            ("token_comment", &$c.token_comment), ("token_function", &$c.token_function),
            ("token_type", &$c.token_type), ("token_variable", &$c.token_variable),
            ("token_number", &$c.token_number), ("token_operator", &$c.token_operator),
            ("token_punctuation", &$c.token_punctuation),
        ]
    };
}

impl Colors {
    /// Every slot of a v1 theme, as a full color set.
    pub fn from_theme(t: &Theme) -> Self {
        let s = |v: &String| Some(v.clone());
        Colors {
            bg_hard: s(&t.bg_hard), bg: s(&t.bg), bg_soft: s(&t.bg_soft), bg1: s(&t.bg1), bg2: s(&t.bg2),
            bg3: s(&t.bg3), bg4: s(&t.bg4), fg: s(&t.fg), fg1: s(&t.fg1), fg2: s(&t.fg2), fg3: s(&t.fg3),
            fg4: s(&t.fg4), accent: s(&t.accent), primary: t.primary.clone(), border: s(&t.border),
            bright_red: s(&t.bright_red), bright_green: s(&t.bright_green), bright_yellow: s(&t.bright_yellow),
            bright_blue: s(&t.bright_blue), bright_purple: s(&t.bright_purple), bright_aqua: s(&t.bright_aqua),
            bright_orange: s(&t.bright_orange), token_keyword: s(&t.token_keyword), token_string: s(&t.token_string),
            token_comment: s(&t.token_comment), token_function: s(&t.token_function), token_type: s(&t.token_type),
            token_variable: s(&t.token_variable), token_number: s(&t.token_number),
            token_operator: s(&t.token_operator), token_punctuation: s(&t.token_punctuation),
        }
    }

    /// `base` with every slot this set defines laid over it.
    pub fn over(&self, mut base: Theme) -> Theme {
        let pick = |v: &Option<String>, b: &mut String| {
            if let Some(v) = v.as_ref().filter(|v| !v.trim().is_empty()) {
                *b = v.trim().to_string();
            }
        };
        pick(&self.bg_hard, &mut base.bg_hard); pick(&self.bg, &mut base.bg); pick(&self.bg_soft, &mut base.bg_soft);
        pick(&self.bg1, &mut base.bg1); pick(&self.bg2, &mut base.bg2); pick(&self.bg3, &mut base.bg3);
        pick(&self.bg4, &mut base.bg4); pick(&self.fg, &mut base.fg); pick(&self.fg1, &mut base.fg1);
        pick(&self.fg2, &mut base.fg2); pick(&self.fg3, &mut base.fg3); pick(&self.fg4, &mut base.fg4);
        pick(&self.accent, &mut base.accent); pick(&self.border, &mut base.border);
        pick(&self.bright_red, &mut base.bright_red); pick(&self.bright_green, &mut base.bright_green);
        pick(&self.bright_yellow, &mut base.bright_yellow); pick(&self.bright_blue, &mut base.bright_blue);
        pick(&self.bright_purple, &mut base.bright_purple); pick(&self.bright_aqua, &mut base.bright_aqua);
        pick(&self.bright_orange, &mut base.bright_orange); pick(&self.token_keyword, &mut base.token_keyword);
        pick(&self.token_string, &mut base.token_string); pick(&self.token_comment, &mut base.token_comment);
        pick(&self.token_function, &mut base.token_function); pick(&self.token_type, &mut base.token_type);
        pick(&self.token_variable, &mut base.token_variable); pick(&self.token_number, &mut base.token_number);
        pick(&self.token_operator, &mut base.token_operator); pick(&self.token_punctuation, &mut base.token_punctuation);
        if let Some(p) = self.primary.as_ref().filter(|v| !v.trim().is_empty()) {
            base.primary = Some(p.trim().to_string());
        }
        base
    }
}

/// Look-and-feel settings beyond color. Fonts, corner radius and motion apply
/// today; the rest are read and kept for the component-style phase.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Style {
    /// UI font stack, e.g. `"Inter", sans-serif`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub font_sans: Option<String>,
    /// Code font stack.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub font_mono: Option<String>,
    /// Base corner radius, e.g. `0.5rem` or `8px`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub radius: Option<String>,
    /// `"full"` (default) or `"reduced"` (no transitions / animations).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub motion: Option<String>,
    /// Border width, e.g. `1px` / `2px`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub border_width: Option<String>,
    /// `"none" | "soft" | "strong" | "glow" | "hard"` (hard = offset, no blur).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub shadow: Option<String>,
    /// `"solid" | "glass"`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub surface: Option<String>,
    /// `"compact" | "normal" | "comfortable"`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub density: Option<String>,
    /// `"solid" | "pill" | "raised" | "brutal" | "glow"`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub button: Option<String>,
    /// App background pattern: `"none" | "grain" | "grid" | "dots" | "scanlines"`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub texture: Option<String>,
}

/// Allowed values for the enum-like style keys.
const STYLE_ENUMS: [(&str, &[&str]); 6] = [
    ("motion", &["full", "reduced"]),
    ("shadow", &["none", "soft", "strong", "glow", "hard"]),
    ("surface", &["solid", "glass"]),
    ("density", &["compact", "normal", "comfortable"]),
    ("button", &["solid", "pill", "raised", "brutal", "glow"]),
    ("texture", &["none", "grain", "grid", "dots", "scanlines"]),
];

/// A theme file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Pack {
    #[serde(default = "default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub dark: Option<Colors>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub light: Option<Colors>,
    #[serde(default)]
    pub style: Style,
    /// Optional custom CSS (scanned; scoped away from protected UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}

fn default_format() -> u32 {
    FORMAT
}

/// Base each mode's missing slots come from: Obsidian, or Basalt's light set.
fn base_for(mode: &str) -> Theme {
    if mode != "light" {
        return Theme::obsidian();
    }
    serde_json::from_str::<Pack>(PACKS[0].1)
        .ok()
        .and_then(|p| p.light)
        .map(|c| c.over(Theme::graphite_light()))
        .unwrap_or_else(Theme::graphite_light)
}

impl Pack {
    /// Pack wrapping a v1 single-mode theme.
    pub fn from_v1(t: &Theme) -> Self {
        let colors = Some(Colors::from_theme(t));
        let light = t.kind.eq_ignore_ascii_case("light");
        Pack {
            format: FORMAT,
            name: t.name.clone(),
            dark: if light { None } else { colors.clone() },
            light: if light { colors } else { None },
            ..Default::default()
        }
    }

    /// Modes this pack provides.
    pub fn modes(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if self.dark.is_some() { m.push("dark"); }
        if self.light.is_some() { m.push("light"); }
        m
    }

    /// Full color theme for `mode` ("dark" / "light"), falling back to the
    /// other mode when this pack only has one.
    pub fn resolve(&self, mode: &str) -> Theme {
        let (kind, colors) = match (mode, &self.dark, &self.light) {
            ("light", _, Some(l)) => ("light", l),
            ("dark", Some(d), _) => ("dark", d),
            (_, Some(d), None) => ("dark", d),
            (_, None, Some(l)) => ("light", l),
            (_, Some(d), Some(_)) => ("dark", d),
            (_, None, None) => {
                let mut t = base_for(mode);
                t.name = self.name.clone();
                return t;
            }
        };
        let mut t = colors.over(base_for(kind));
        t.name = self.name.clone();
        t.kind = kind.to_string();
        t
    }
}

/// Parse a theme file: format 2 pack, or a v1 palette (JSON or TOML).
pub fn parse(text: &str) -> Result<Pack, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err(format!("Theme file is too large (max {} KB)", MAX_FILE_BYTES / 1024));
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| format!("Invalid theme JSON: {e}"))?;
        let is_v2 = v.get("format").is_some() || v.get("dark").is_some() || v.get("light").is_some() || v.get("style").is_some() || v.get("css").is_some();
        let pack = if is_v2 {
            serde_json::from_value::<Pack>(v).map_err(|e| format!("Invalid theme: {e}"))?
        } else {
            Pack::from_v1(&serde_json::from_value::<Theme>(v).map_err(|e| format!("Invalid palette: {e}"))?)
        };
        validate(&pack)?;
        return Ok(pack);
    }
    let pack = Pack::from_v1(&Theme::from_toml(text)?);
    validate(&pack)?;
    Ok(pack)
}

/// Whether `v` looks like a single CSS color (hex, rgb/hsl/oklch/lab/…, a
/// named color, `transparent`) — never anything that could carry other CSS.
pub fn is_safe_color(v: &str) -> bool {
    let v = v.trim();
    if v.is_empty() || v.len() > 80 {
        return false;
    }
    if v.contains(['{', '}', ';', '<', '>', '\\', '"', '\'', '@', '!']) || v.to_ascii_lowercase().contains("url") {
        return false;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let lower = v.to_ascii_lowercase();
    let funcs = ["rgb(", "rgba(", "hsl(", "hsla(", "hwb(", "lab(", "lch(", "oklab(", "oklch(", "color(", "color-mix("];
    if funcs.iter().any(|f| lower.starts_with(f)) {
        return lower.ends_with(')') && lower.chars().all(|c| c.is_ascii_alphanumeric() || " .,%/()-+#".contains(c));
    }
    lower.chars().all(|c| c.is_ascii_alphabetic())
}

/// Whether a style value (font stack, length, keyword) is plain enough to
/// set as a CSS custom property.
fn is_safe_style_value(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty()
        && v.len() <= 200
        && !v.to_ascii_lowercase().contains("url")
        && v.chars().all(|c| c.is_ascii_alphanumeric() || " .,%-_()'\"/".contains(c))
}

/// Reject packs whose metadata / colors / style values could inject CSS.
pub fn validate(p: &Pack) -> Result<(), String> {
    if p.format > FORMAT {
        return Err(format!("This theme needs a newer Rustic (format {}, supported {FORMAT})", p.format));
    }
    let name = p.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("The theme needs a name (up to 80 characters)".into());
    }
    if p.dark.is_none() && p.light.is_none() && p.css.as_deref().is_none_or(|c| c.trim().is_empty()) {
        return Err("The theme defines no colors and no CSS".into());
    }
    for (mode, colors) in [("dark", &p.dark), ("light", &p.light)] {
        let Some(c) = colors else { continue };
        for (field, value) in color_fields!(c) {
            if let Some(v) = value {
                if !is_safe_color(v) {
                    return Err(format!("{mode}.{field}: \"{v}\" isn't a valid color"));
                }
            }
        }
    }
    let s = &p.style;
    for (field, value) in [
        ("font_sans", &s.font_sans), ("font_mono", &s.font_mono), ("radius", &s.radius),
        ("motion", &s.motion), ("border_width", &s.border_width), ("shadow", &s.shadow),
        ("surface", &s.surface), ("density", &s.density), ("button", &s.button), ("texture", &s.texture),
    ] {
        if let Some(v) = value {
            if !is_safe_style_value(v) {
                return Err(format!("style.{field}: \"{v}\" isn't allowed"));
            }
            if let Some((_, allowed)) = STYLE_ENUMS.iter().find(|(f, _)| *f == field) {
                if !allowed.contains(&v.trim()) {
                    return Err(format!("style.{field}: \"{v}\" — use one of: {}", allowed.join(", ")));
                }
            }
        }
    }
    Ok(())
}

/// One scanner result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub code: String,
    pub message: String,
}

/// What the scanner found in a pack.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScanReport {
    /// Patterns that make the theme impossible to trust.
    pub blocked: Vec<Finding>,
    /// Risky patterns the user should know about before trusting.
    pub warnings: Vec<Finding>,
    /// Hosts the theme's CSS loads from (`@import`, `url()`).
    pub remote_hosts: Vec<String>,
    pub css_bytes: usize,
}

/// CSS with `/* … */` comments removed (so patterns can't hide in them).
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Every `url(...)` / `@import "..."` target in `css`.
fn css_urls(css: &str) -> Vec<String> {
    let lower = css.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = lower[i..].find("url(") {
        let start = i + pos + 4;
        let end = lower[start..].find(')').map(|e| start + e).unwrap_or(lower.len());
        out.push(lower[start..end].trim().trim_matches(['"', '\'']).trim().to_string());
        i = end.min(lower.len());
        if i >= lower.len() { break; }
    }
    let mut j = 0;
    while let Some(pos) = lower[j..].find("@import") {
        let start = j + pos + 7;
        let seg_end = lower[start..].find(';').map(|e| start + e).unwrap_or(lower.len());
        let seg = lower[start..seg_end].trim();
        if let Some(q) = seg.chars().next().filter(|c| *c == '"' || *c == '\'') {
            if let Some(close) = seg[1..].find(q) {
                out.push(seg[1..1 + close].to_string());
            }
        }
        j = seg_end.min(lower.len());
        if j >= lower.len() { break; }
    }
    out
}

/// Host of an http(s) URL.
fn url_host(u: &str) -> Option<String> {
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).or_else(|| u.strip_prefix("//"))?;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// Relative luminance of a `#rrggbb` color (other formats: `None`).
fn hex_luminance(c: &str) -> Option<f64> {
    let h = c.trim().strip_prefix('#')?;
    let h: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => h[..6].to_string(),
        _ => return None,
    };
    let ch = |i: usize| -> Option<f64> {
        let v = u8::from_str_radix(&h[i..i + 2], 16).ok()? as f64 / 255.0;
        Some(if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
    };
    Some(0.2126 * ch(0)? + 0.7152 * ch(2)? + 0.0722 * ch(4)?)
}

/// WCAG contrast of two hex colors, when both are hex.
fn contrast(a: &str, b: &str) -> Option<f64> {
    let (x, y) = (hex_luminance(a)?, hex_luminance(b)?);
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    Some((hi + 0.05) / (lo + 0.05))
}

/// Scan a pack's CSS and colors.
pub fn scan(p: &Pack) -> ScanReport {
    let mut r = ScanReport::default();
    let mut block = |code: &str, msg: String| r.blocked.push(Finding { code: code.into(), message: msg });
    let raw = p.css.clone().unwrap_or_default();
    let css = strip_comments(&raw);
    let lower = css.to_ascii_lowercase();
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();

    for (needle, code, msg) in [
        ("javascript:", "script-url", "Contains a javascript: URL"),
        ("vbscript:", "script-url", "Contains a vbscript: URL"),
        ("expression(", "css-expression", "Uses expression(), which can run script in old engines"),
        ("behavior:", "css-behavior", "Uses behavior:, which can load script in old engines"),
        ("-moz-binding", "css-binding", "Uses -moz-binding, which can load script"),
        ("<script", "html-in-css", "Contains a <script> tag"),
        ("</style", "html-in-css", "Tries to close the style block"),
        ("data-rustic-protected", "protected-ui", "Targets Rustic's protected prompts"),
    ] {
        if compact.contains(&needle.replace(' ', "")) {
            block(code, msg.to_string());
        }
    }
    let mut hosts = std::collections::BTreeSet::new();
    let mut warns: Vec<Finding> = Vec::new();
    let mut warn = |code: &str, msg: String| {
        if !warns.iter().any(|w| w.code == code && w.message == msg) {
            warns.push(Finding { code: code.into(), message: msg });
        }
    };
    for u in css_urls(&css) {
        if u.starts_with("data:") {
            let mime = u[5..].split([';', ',']).next().unwrap_or_default();
            let ok = mime.starts_with("image/") && mime != "image/svg+xml" || mime.starts_with("font/") || mime.starts_with("application/font") || mime.starts_with("application/x-font");
            if !ok {
                block("data-url", format!("Embeds data of type \"{mime}\" (only images and fonts are allowed)"));
            }
        } else if u.starts_with("http://") || u.starts_with("https://") || u.starts_with("//") {
            if let Some(h) = url_host(&u) {
                hosts.insert(h);
            }
            if u.starts_with("http://") {
                warn("insecure-url", format!("Loads over plain HTTP: {u}"));
            }
        } else if u.contains(':') {
            block("url-scheme", format!("Loads from an unsupported location: {u}"));
        }
    }
    if !hosts.is_empty() {
        warn("remote", format!("Loads files from the internet: {}", hosts.iter().cloned().collect::<Vec<_>>().join(", ")));
    }
    if compact.contains("@import") {
        warn("import", "Imports other stylesheets (their contents are not scanned)".into());
    }
    let broad = ["*{", "*,", "body{", "html{", ":root{", "body>", "body*", "[role=dialog]", "[role=\"dialog\"]", "dialog{", "[data-slot=dialog"];
    let hides = ["display:none", "visibility:hidden", "opacity:0;", "opacity:0}", "pointer-events:none", "clip-path:", "transform:scale(0"];
    if broad.iter().any(|b| compact.contains(b)) && hides.iter().any(|h| compact.contains(h)) {
        warn("hide", "May hide or block large parts of the app (broad selectors with display/visibility/opacity/pointer-events)".into());
    }
    let overlays = compact.contains("position:fixed") && (compact.contains("z-index:") && lower.split("z-index").skip(1).any(|s| {
        s.trim_start_matches([':', ' ']).chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().is_ok_and(|n| n >= 1000)
    }));
    if overlays {
        warn("overlay", "Creates fixed overlays with a very high z-index (could cover the app)".into());
    }
    if compact.contains("content:") && lower.matches("content").count() > 20 {
        warn("content", "Injects a lot of text via content: (could imitate UI)".into());
    }
    if raw.len() > 200 * 1024 {
        warn("size", format!("Large stylesheet ({} KB)", raw.len() / 1024));
    }
    for (mode, colors) in [("dark", &p.dark), ("light", &p.light)] {
        if colors.is_none() { continue; }
        let t = p.resolve(mode);
        if let Some(c) = contrast(&t.fg, &t.bg).filter(|c| *c < 4.5) {
            warn("contrast", format!("{mode}: text on background has low contrast ({c:.1}:1, aim for 4.5:1)"));
        }
        if let Some(c) = contrast(&t.fg3, &t.bg).filter(|c| *c < 3.0) {
            warn("contrast", format!("{mode}: muted text is hard to read ({c:.1}:1)"));
        }
    }
    r.warnings = warns;
    r.remote_hosts = hosts.into_iter().collect();
    r.css_bytes = raw.len();
    r
}

/// Custom CSS ready to inject: `@import` statements and `@font-face` /
/// `@keyframes` / `@property` blocks hoisted to the top level, everything else
/// wrapped in `@scope (:root) to ([data-rustic-protected])`.
pub fn prepare_css(css: &str) -> String {
    let css = strip_comments(css);
    let bytes = css.as_bytes();
    let mut imports = String::new();
    let mut hoisted = String::new();
    let mut body = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &css[i..];
        let trimmed = rest.trim_start();
        let lead = rest.len() - trimmed.len();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("@import") || lower.starts_with("@charset") {
            let end = trimmed.find(';').map(|e| e + 1).unwrap_or(trimmed.len());
            if lower.starts_with("@import") {
                imports.push_str(trimmed[..end].trim());
                imports.push('\n');
            }
            i += lead + end;
            continue;
        }
        let hoist = ["@font-face", "@keyframes", "@-webkit-keyframes", "@property", "@counter-style"]
            .iter()
            .any(|a| lower.starts_with(a));
        // Find the end of this top-level statement / block.
        let mut depth = 0i32;
        let mut end = trimmed.len();
        let mut seen_brace = false;
        for (k, ch) in trimmed.char_indices() {
            match ch {
                '{' => { depth += 1; seen_brace = true; }
                '}' => {
                    depth -= 1;
                    if depth <= 0 && seen_brace { end = k + 1; break; }
                }
                ';' if depth == 0 && !seen_brace => { end = k + 1; break; }
                _ => {}
            }
        }
        if end == 0 { break; }
        let chunk = &trimmed[..end];
        if hoist { hoisted.push_str(chunk); hoisted.push('\n'); } else { body.push_str(chunk); body.push('\n'); }
        i += lead + end;
    }
    let mut out = String::new();
    out.push_str(&imports);
    out.push_str(&hoisted);
    if !body.trim().is_empty() {
        out.push_str("@scope (:root) to ([data-rustic-protected]) {\n");
        out.push_str(&body);
        out.push_str("}\n");
    }
    out
}

/// One theme in the list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Built-in name, or the file's id (file name without the extension).
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub modes: Vec<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub has_css: bool,
    pub trusted: bool,
    /// Swatches for the cards: `[bg, bg1, fg, primary/accent]` per mode.
    pub swatches: BTreeMap<String, Vec<String>>,
    pub file: Option<String>,
    /// Style recipes, so cards can preview shape / shadow / button style.
    pub style: Style,
}

/// `<data>/themes`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("themes")
}

/// File for theme `id`.
fn file_for(data_dir: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("Invalid theme id: {id}"));
    }
    Ok(dir(data_dir).join(format!("{id}{EXT}")))
}

/// A file-name-safe id for `name`.
fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "theme".into() } else { s.chars().take(48).collect() }
}

/// sha-256 of `bytes` (hex).
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn read_settings(state: &AppState) -> rustic_core::config::UserSettings {
    let db = state.db.lock_safe();
    db.get_setting("user_settings")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn write_settings(state: &AppState, s: &rustic_core::config::UserSettings) -> Result<(), String> {
    let json = serde_json::to_string(s).map_err(|e| e.to_string())?;
    state.db.lock_safe().set_setting("user_settings", &json).map_err(|e| e.to_string())
}

fn read_trust(state: &AppState) -> BTreeMap<String, String> {
    state
        .db
        .lock_safe()
        .get_setting(TRUST_KEY)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn write_trust(state: &AppState, t: &BTreeMap<String, String>) -> Result<(), String> {
    let json = serde_json::to_string(t).map_err(|e| e.to_string())?;
    state.db.lock_safe().set_setting(TRUST_KEY, &json).map_err(|e| e.to_string())
}

/// Move palettes imported before theme files existed (DB `theme:<name>`) into
/// files, keeping them trusted (the user installed them under the old flow).
fn migrate_legacy(state: &AppState, data_dir: &Path) {
    let mut settings = read_settings(state);
    if settings.theme.custom_themes.is_empty() {
        return;
    }
    let mut trust = read_trust(state);
    let mut remaining = Vec::new();
    for name in std::mem::take(&mut settings.theme.custom_themes) {
        let key = format!("theme:{name}");
        let legacy = state.db.lock_safe().get_setting(&key).ok().flatten();
        let Some(json) = legacy else { continue };
        let Ok(theme) = serde_json::from_str::<Theme>(&json) else {
            remaining.push(name);
            continue;
        };
        let pack = Pack::from_v1(&theme);
        match save_new(state, data_dir, &pack, true) {
            Ok((id, h)) => {
                trust.insert(id.clone(), h);
                if settings.theme.active_theme == name {
                    settings.theme.active_theme = id;
                }
                let _ = state.db.lock_safe().delete_setting(&key);
            }
            Err(e) => {
                tracing::warn!(name, "theme migration failed: {e}");
                remaining.push(name);
            }
        }
    }
    settings.theme.custom_themes = remaining;
    let _ = write_trust(state, &trust);
    let _ = write_settings(state, &settings);
}

/// Pretty JSON of a pack (the on-disk form).
pub fn to_file_text(p: &Pack) -> Result<String, String> {
    serde_json::to_string_pretty(p).map_err(|e| e.to_string())
}

/// Write a new theme file (unique id from its name). Returns `(id, hash)`.
fn save_new(_state: &AppState, data_dir: &Path, p: &Pack, _migrating: bool) -> Result<(String, String), String> {
    let d = dir(data_dir);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let base = slug(&p.name);
    let mut id = base.clone();
    let mut n = 2;
    while file_for(data_dir, &id)?.exists() || is_builtin(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let text = to_file_text(p)?;
    std::fs::write(file_for(data_dir, &id)?, &text).map_err(|e| e.to_string())?;
    Ok((id, hash(text.as_bytes())))
}

/// A theme file's text, pack and hash.
pub fn read_file(data_dir: &Path, id: &str) -> Result<(String, Pack, String), String> {
    let path = file_for(data_dir, id)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read theme {id}: {e}"))?;
    let pack = parse(&text)?;
    let h = hash(text.as_bytes());
    Ok((text, pack, h))
}

/// Built-in theme files (dark + light each), listed after Obsidian. Generated
/// from OKLCH ladders with contrast targets; also usable as editable examples.
const PACKS: &[(&str, &str)] = &[
    ("Basalt", include_str!("../themes/basalt.rustic-theme.json")),
    ("Hearth", include_str!("../themes/hearth.rustic-theme.json")),
    ("Petal", include_str!("../themes/petal.rustic-theme.json")),
    ("Midnight", include_str!("../themes/midnight.rustic-theme.json")),
    ("Onyx", include_str!("../themes/onyx.rustic-theme.json")),
    ("Neo Brutal", include_str!("../themes/neo-brutal.rustic-theme.json")),
    ("Grove", include_str!("../themes/grove.rustic-theme.json")),
    ("Neon Grid", include_str!("../themes/neon-grid.rustic-theme.json")),
];

/// Retired built-in ids → (replacement id, color mode to switch to).
const RENAMED: &[(&str, &str, &str)] = &[
    ("Luxide Dark", "Midnight", "dark"),
    ("Verdigris", "Basalt", "dark"),
    ("Graphite", "Basalt", "dark"),
    ("Graphite Light", "Basalt", "light"),
    ("Ink", "Hearth", "dark"),
    ("Paper", "Hearth", "light"),
    ("Fjord Night", "Midnight", "dark"),
    ("Fjord Day", "Midnight", "light"),
    ("Kiln", "Hearth", "dark"),
    ("Kiln Light", "Hearth", "light"),
];

/// Replacement `(id, mode)` when `id` is a retired built-in.
fn renamed(id: &str) -> Option<(&'static str, &'static str)> {
    RENAMED.iter().find(|r| r.0 == id).map(|r| (r.1, r.2))
}

/// Whether `id` names a built-in theme (current or retired).
fn is_builtin(id: &str) -> bool {
    builtin_pack(id).is_some()
}

/// Built-in pack for `name`: Obsidian (dark only) or one of [`PACKS`].
fn builtin_pack(name: &str) -> Option<Pack> {
    let name = renamed(name).map(|r| r.0).unwrap_or(name);
    if let Some((_, text)) = PACKS.iter().find(|(id, _)| *id == name) {
        return parse(text).ok();
    }
    (name == "Obsidian").then(|| Pack::from_v1(&Theme::obsidian()))
}

/// A complete starter theme file (every color slot and style key filled in) to copy and edit.
pub fn template() -> String {
    let pack = Pack {
        format: FORMAT,
        name: "My Theme".into(),
        author: Some("Your name".into()),
        version: Some("1.0.0".into()),
        description: Some("Starter template: edit the values, then import this file in Settings > Appearance > Themes. Delete any key to inherit Rustic's default.".into()),
        dark: Some(Colors::from_theme(&Theme::obsidian())),
        light: Some(Colors::from_theme(&base_for("light"))),
        style: Style {
            font_sans: Some("\"Inter\", sans-serif".into()),
            font_mono: Some("\"Victor Mono\", monospace".into()),
            radius: Some("0.5rem".into()),
            motion: Some("full".into()),
            border_width: Some("1px".into()),
            shadow: Some("soft".into()),
            surface: Some("solid".into()),
            density: Some("normal".into()),
            button: Some("solid".into()),
            texture: Some("none".into()),
        },
        css: Some(String::new()),
    };
    serde_json::to_string_pretty(&pack).unwrap_or_default()
}

fn swatches(p: &Pack) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    for mode in p.modes() {
        let t = p.resolve(mode);
        m.insert(mode.to_string(), vec![t.bg.clone(), t.bg1.clone(), t.fg.clone(), t.primary.clone().unwrap_or(t.accent.clone())]);
    }
    m
}

/// Built-in + installed themes.
pub fn list(state: &AppState, data_dir: &Path) -> Vec<Entry> {
    migrate_legacy(state, data_dir);
    let trust = read_trust(state);
    let mut out: Vec<Entry> = std::iter::once("Obsidian")
        .chain(PACKS.iter().map(|(id, _)| *id))
        .filter_map(|n| builtin_pack(n).map(|p| (n, p)))
        .map(|(n, p)| Entry {
            id: n.to_string(),
            name: p.name.clone(),
            builtin: true,
            modes: p.modes().iter().map(|m| m.to_string()).collect(),
            author: Some("Rustic".into()),
            version: None,
            description: p.description.clone(),
            has_css: false,
            trusted: true,
            swatches: swatches(&p),
            file: None,
            style: p.style.clone(),
        })
        .collect();
    if let Ok(rd) = std::fs::read_dir(dir(data_dir)) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(EXT)).collect();
        files.sort();
        for path in files {
            let id = path.file_name().map(|n| n.to_string_lossy().trim_end_matches(EXT).to_string()).unwrap_or_default();
            let Ok((_, pack, h)) = read_file(data_dir, &id) else { continue };
            out.push(Entry {
                trusted: trust.get(&id) == Some(&h),
                id,
                name: pack.name.clone(),
                builtin: false,
                modes: pack.modes().iter().map(|m| m.to_string()).collect(),
                author: pack.author.clone(),
                version: pack.version.clone(),
                description: pack.description.clone(),
                has_css: pack.css.as_deref().is_some_and(|c| !c.trim().is_empty()),
                swatches: swatches(&pack),
                file: Some(path.to_string_lossy().into_owned()),
                style: pack.style.clone(),
            });
        }
    }
    out
}

/// Full detail of one theme (for the trust prompt / editor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detail {
    pub entry: Entry,
    pub pack: Pack,
    pub text: String,
    pub hash: String,
    pub scan: ScanReport,
}

/// Theme `id` with its file text, hash and scan.
pub fn get(state: &AppState, data_dir: &Path, id: &str) -> Result<Detail, String> {
    if let Some(p) = builtin_pack(id) {
        let text = to_file_text(&p)?;
        let entry = list(state, data_dir).into_iter().find(|e| e.id == id).ok_or("unknown theme")?;
        return Ok(Detail { entry, scan: scan(&p), hash: hash(text.as_bytes()), pack: p, text });
    }
    let (text, pack, h) = read_file(data_dir, id)?;
    let entry = list(state, data_dir).into_iter().find(|e| e.id == id).ok_or("unknown theme")?;
    Ok(Detail { entry, scan: scan(&pack), pack, text, hash: h })
}

/// Install theme text as a new file (always untrusted). Returns its detail.
pub fn import_text(state: &AppState, data_dir: &Path, text: &str) -> Result<Detail, String> {
    let pack = parse(text)?;
    let (id, _) = save_new(state, data_dir, &pack, false)?;
    tracing::info!(id, name = %pack.name, "theme imported");
    get(state, data_dir, &id)
}

/// Install a theme from a file on this machine.
pub fn import_path(state: &AppState, data_dir: &Path, path: &str) -> Result<Detail, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    if meta.len() as usize > MAX_FILE_BYTES {
        return Err(format!("Theme file is too large (max {} KB)", MAX_FILE_BYTES / 1024));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    import_text(state, data_dir, &text)
}

/// Replace theme `id`'s file with `text` (validated). Trust is dropped: an
/// edited theme must be trusted again before it applies.
pub fn write(state: &AppState, data_dir: &Path, id: &str, text: &str) -> Result<Detail, String> {
    if is_builtin(id) {
        return Err("Built-in themes can't be edited — import a copy instead".into());
    }
    let path = file_for(data_dir, id)?;
    if !path.exists() {
        return Err(format!("No theme {id}"));
    }
    parse(text)?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    get(state, data_dir, id)
}

/// Delete theme `id` (falls back to the default theme if it was active).
pub fn delete(state: &AppState, data_dir: &Path, id: &str) -> Result<(), String> {
    if is_builtin(id) {
        return Err("Built-in themes can't be deleted".into());
    }
    let path = file_for(data_dir, id)?;
    std::fs::remove_file(&path).map_err(|e| format!("Couldn't delete theme {id}: {e}"))?;
    let mut trust = read_trust(state);
    trust.remove(id);
    write_trust(state, &trust)?;
    let mut s = read_settings(state);
    if s.theme.active_theme == id {
        s.theme.active_theme = "Obsidian".into();
        write_settings(state, &s)?;
    }
    Ok(())
}

/// Trust theme `id` — only if its file still has the `hash` the user reviewed.
pub fn trust(state: &AppState, data_dir: &Path, id: &str, reviewed_hash: &str) -> Result<(), String> {
    let (_, pack, h) = read_file(data_dir, id)?;
    if h != reviewed_hash {
        return Err("The theme file changed after you reviewed it — review it again".into());
    }
    let report = scan(&pack);
    if let Some(b) = report.blocked.first() {
        return Err(format!("This theme can't be trusted: {}", b.message));
    }
    let mut t = read_trust(state);
    t.insert(id.to_string(), h);
    write_trust(state, &t)
}

/// Make `id` the active theme (must be built-in or trusted).
pub fn apply(state: &AppState, data_dir: &Path, id: &str) -> Result<(), String> {
    if !is_builtin(id) {
        let (_, _, h) = read_file(data_dir, id)?;
        if read_trust(state).get(id) != Some(&h) {
            return Err("Trust this theme first".into());
        }
    }
    let mut s = read_settings(state);
    match renamed(id) {
        Some((new_id, mode)) => {
            s.theme.active_theme = new_id.to_string();
            s.theme.mode = mode.into();
        }
        None => s.theme.active_theme = id.to_string(),
    }
    write_settings(state, &s)
}

/// Set the color mode: `"dark"`, `"light"` or `"system"`.
pub fn set_mode(state: &AppState, mode: &str) -> Result<(), String> {
    if !matches!(mode, "dark" | "light" | "system") {
        return Err("mode must be dark, light or system".into());
    }
    let mut s = read_settings(state);
    s.theme.mode = mode.to_string();
    write_settings(state, &s)
}

/// What the UI paints: resolved colors per available mode, style, and the
/// prepared custom CSS (only for built-in / trusted themes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Active {
    pub id: String,
    pub name: String,
    pub mode: String,
    pub dark: Option<Theme>,
    pub light: Option<Theme>,
    pub style: Style,
    pub css: Option<String>,
    /// The active theme was untrusted / missing, so the default is shown.
    pub fallback: bool,
}

/// The active theme, ready to paint.
pub fn active(state: &AppState, data_dir: &Path) -> Active {
    migrate_legacy(state, data_dir);
    let mut s = read_settings(state);
    if let Some((new_id, m)) = renamed(&s.theme.active_theme) {
        s.theme.active_theme = new_id.to_string();
        s.theme.mode = m.into();
        let _ = write_settings(state, &s);
    }
    let mode = if s.theme.mode.is_empty() { "dark".to_string() } else { s.theme.mode.clone() };
    let id = s.theme.active_theme.clone();
    let (pack, fallback) = match builtin_pack(&id) {
        Some(p) => (p, false),
        None => match read_file(data_dir, &id) {
            Ok((_, p, h)) if read_trust(state).get(&id) == Some(&h) && scan(&p).blocked.is_empty() => (p, false),
            _ => (builtin_pack("Obsidian").unwrap_or_default(), true),
        },
    };
    let css = pack.css.as_deref().filter(|c| !c.trim().is_empty()).map(prepare_css);
    // A single-mode theme still switches: the missing mode uses the default palette.
    let variant = |m: &str, has: bool| Some(if has || pack.modes().is_empty() { pack.resolve(m) } else { base_for(m) });
    Active {
        id: if fallback { "Obsidian".into() } else { id },
        name: pack.name.clone(),
        mode,
        dark: variant("dark", pack.dark.is_some()),
        light: variant("light", pack.light.is_some()),
        style: pack.style.clone(),
        css,
        fallback,
    }
}

/// Theme store for the agent's theme tools, over a peer host (desktop or server).
pub struct AgentThemeHost {
    host: crate::peer::HostRef,
}

impl AgentThemeHost {
    /// Register the store the agent's `*_theme` tools use.
    pub fn register(host: crate::peer::HostRef) {
        rustic_agent::tools::theme_tools::set_theme_host(std::sync::Arc::new(AgentThemeHost { host }));
    }
}

impl rustic_agent::tools::theme_tools::ThemeHost for AgentThemeHost {
    fn list(&self) -> Result<serde_json::Value, String> {
        let dir = self.host.data_dir()?;
        let state = self.host.state();
        let a = active(state, &dir);
        Ok(serde_json::json!({ "active": a.id, "mode": a.mode, "themes": list(state, &dir) }))
    }

    fn read(&self, id: &str) -> Result<serde_json::Value, String> {
        let d = get(self.host.state(), &self.host.data_dir()?, id)?;
        Ok(serde_json::json!({ "entry": d.entry, "text": d.text, "scan": d.scan }))
    }

    fn template(&self) -> String {
        template()
    }

    fn write(&self, id: Option<&str>, text: &str) -> Result<serde_json::Value, String> {
        let dir = self.host.data_dir()?;
        let state = self.host.state();
        let d = match id {
            Some(id) if is_builtin(id) => {
                return Err(format!("`{id}` is a built-in theme and can't be changed — omit `id` to save your version as a new theme"))
            }
            Some(id) => write(state, &dir, id, text)?,
            None => import_text(state, &dir, text)?,
        };
        use crate::context::EventEmitterExt;
        self.host.emitter().emit("theme-agent-saved", serde_json::json!({ "id": d.entry.id, "name": d.entry.name }));
        Ok(serde_json::json!({ "entry": d.entry, "scan": d.scan }))
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        delete(self.host.state(), &self.host.data_dir()?, id)?;
        use crate::context::EventEmitterExt;
        self.host.emitter().emit("theme-agent-saved", serde_json::json!({ "id": id, "deleted": true }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_reject_injection() {
        assert!(is_safe_color("#1a2b3c"));
        assert!(is_safe_color("oklch(0.5 0.1 200)"));
        assert!(is_safe_color("rebeccapurple"));
        assert!(is_safe_color("color-mix(in oklab, #fff 10%, transparent)"));
        assert!(!is_safe_color("#fff; background: url(http://x)"));
        assert!(!is_safe_color("red}body{display:none"));
        assert!(!is_safe_color("url(x)"));
        assert!(!is_safe_color("#12345"));
    }

    #[test]
    fn v1_palettes_parse_and_v2_resolves_with_fallbacks() {
        let v1 = serde_json::to_string(&Theme::verdigris()).unwrap();
        let p = parse(&v1).unwrap();
        assert_eq!(p.modes(), vec!["dark"]);
        assert_eq!(p.resolve("light").bg, Theme::verdigris().bg, "single-mode pack uses its only mode");

        let v2 = r##"{ "format": 2, "name": "Duo", "dark": { "bg": "#101010", "primary": "#ff8800" }, "light": { "bg": "#fafafa" } }"##;
        let p = parse(v2).unwrap();
        let d = p.resolve("dark");
        assert_eq!((d.bg.as_str(), d.primary.as_deref(), d.kind.as_str()), ("#101010", Some("#ff8800"), "dark"));
        assert_eq!(d.fg, Theme::obsidian().fg, "missing slots come from the dark base");
        let l = p.resolve("light");
        assert_eq!((l.bg.as_str(), l.fg.as_str(), l.kind.as_str()), ("#fafafa", base_for("light").fg.as_str(), "light"));
        assert!(parse(r##"{ "format": 2, "name": "Bad", "dark": { "bg": "#000;}*{display:none" } }"##).is_err());
        assert!(parse(r#"{ "format": 9, "name": "Future", "css": "a{}" }"#).is_err());
    }

    #[test]
    fn scanner_blocks_script_and_warns_on_risky_css() {
        let pack = |css: &str| Pack { name: "T".into(), css: Some(css.into()), ..Default::default() };
        assert_eq!(scan(&pack("a { background: url(javascript:alert(1)) }")).blocked[0].code, "script-url");
        assert_eq!(scan(&pack("a { width: expression(alert(1)) }")).blocked[0].code, "css-expression");
        assert!(!scan(&pack("[data-rustic-protected] { display:none }")).blocked.is_empty());
        assert!(!scan(&pack("a { background: url('data:text/html,<b>') }")).blocked.is_empty());
        assert!(!scan(&pack("a { background: url(file:///etc/passwd) }")).blocked.is_empty());
        assert!(scan(&pack("/* javascript: in a comment is fine */ a { color: red }")).blocked.is_empty());

        let r = scan(&pack("@import url('https://fonts.example.com/x.css'); body { display: none } .x { position: fixed; z-index: 99999 }"));
        assert!(r.blocked.is_empty());
        let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
        assert!(codes.contains(&"remote") && codes.contains(&"import") && codes.contains(&"hide") && codes.contains(&"overlay"), "{codes:?}");
        assert_eq!(r.remote_hosts, vec!["fonts.example.com"]);
        assert!(scan(&pack("a { background: url(data:image/png;base64,AAAA) }")).blocked.is_empty());

        let low = Pack { name: "Low".into(), dark: Some(Colors { bg: Some("#777777".into()), fg: Some("#888888".into()), ..Default::default() }), ..Default::default() };
        assert!(scan(&low).warnings.iter().any(|w| w.code == "contrast"));
    }

    #[test]
    fn prepared_css_hoists_imports_and_scopes_rules() {
        let out = prepare_css("@import url(https://x/y.css);\n/* c */ .a { color: red }\n@font-face { font-family: F; src: url(https://x/f.woff2) }\n@media (min-width: 1px) { .b { color: blue } }\n@keyframes k { from { opacity: 0 } to { opacity: 1 } }");
        let import_at = out.find("@import").unwrap();
        let scope_at = out.find("@scope").unwrap();
        assert!(import_at < scope_at, "{out}");
        assert!(out.find("@font-face").unwrap() < scope_at && out.find("@keyframes").unwrap() < scope_at);
        let scoped = &out[scope_at..];
        assert!(scoped.contains(".a { color: red }") && scoped.contains("@media"));
        assert!(scoped.starts_with("@scope (:root) to ([data-rustic-protected])"));
        assert!(!out.contains("/* c */"));
    }

    #[test]
    fn families_carry_both_modes_and_template_is_valid() {
        let p = builtin_pack("Paper").unwrap();
        assert_eq!(p.name, "Hearth", "retired ids resolve to their replacement");
        assert_eq!(p.modes(), vec!["dark", "light"]);
        assert_eq!(renamed("Kiln Light"), Some(("Hearth", "light")));
        assert!(is_builtin("Obsidian") && is_builtin("Onyx") && !is_builtin("Nope"));
        assert_ne!(base_for("light").bg, Theme::graphite_light().bg, "default light base is Basalt light");
        let t = parse(&template()).unwrap();
        assert_eq!(t.modes(), vec!["dark", "light"]);
        assert!(scan(&t).blocked.is_empty());
        assert!(t.css.as_deref().is_some_and(|c| c.is_empty()));
    }

    #[test]
    fn starters_are_valid_builtins_and_style_enums_are_checked() {
        for (id, _) in PACKS {
            let p = builtin_pack(id).unwrap_or_else(|| panic!("{id} must parse"));
            assert_eq!(p.modes(), vec!["dark", "light"], "{id}");
            let r = scan(&p);
            assert!(r.blocked.is_empty() && !r.warnings.iter().any(|w| w.code == "contrast"), "{id}: {:?}", r.warnings);
            assert!(is_builtin(id));
        }
        assert!(parse(r#"{ "format": 2, "name": "X", "css": "a{}", "style": { "button": "wobbly" } }"#).is_err());
        assert!(parse(r#"{ "format": 2, "name": "X", "css": "a{}", "style": { "texture": "grid", "shadow": "hard" } }"#).is_ok());
    }

    #[test]
    fn slugs_are_file_safe() {
        assert_eq!(slug("Bold & Organic!"), "bold-organic");
        assert_eq!(slug("   "), "theme");
        assert!(file_for(Path::new("d"), "../x").is_err());
    }
}
