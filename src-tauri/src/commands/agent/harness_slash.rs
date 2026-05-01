//! List Claude Code's slash commands so the chat input can autocomplete them.
//!
//! Claude Code reads custom slash commands from two locations:
//! * `~/.claude/commands/<name>.md`     — user-global, available everywhere
//! * `<project>/.claude/commands/<name>.md` — project-scoped, overrides user
//!
//! Each file's name (without `.md`) is the command; the body's H1 (or first
//! non-blank line) is the description we surface in the picker.
//!
//! On top of those we expose a baseline of built-in CLI commands (`/clear`,
//! `/help`, etc.) so the user sees the standard set even on a clean install.
//!
//! No backend execution: Rustic forwards the literal `/foo args` string to
//! the CLI's stdin via the existing user-message path, and Claude Code
//! expands it itself. This module is purely discoverability.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSlashCommand {
    /// Command name without the leading `/`. Insertion uses `/{name} `.
    pub name: String,
    /// One-line description for the picker. Empty string when we couldn't
    /// extract one from the markdown.
    pub description: String,
    /// Where the command came from — surfaced in the picker subtitle so the
    /// user knows whether they're calling a user-global, project-local, or
    /// builtin command.
    pub source: ClaudeSlashSource,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeSlashSource {
    Builtin,
    User,
    Project,
}

/// Builtin CLI commands. These don't live as files anywhere; we hardcode the
/// list so the picker has something to show on a clean install. Names are
/// based on Claude Code's published help output — if the CLI gains new ones,
/// the picker just won't surface them until we update this list (no harm).
const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("clear",         "Clear the conversation history"),
    ("compact",       "Compact the conversation context"),
    ("help",          "List available commands"),
    ("cost",          "Show token usage and cost"),
    ("init",          "Initialise a CLAUDE.md for this project"),
    ("agents",        "Manage subagents"),
    ("mcp",           "Show MCP server status"),
    ("model",         "Switch model"),
    ("permissions",   "View / edit permission rules"),
    ("resume",        "Resume a previous session"),
    ("rewind",        "Undo recent file changes"),
    ("release-notes", "Show recent release notes"),
];

/// Tauri command. `project_root` is optional; pass `None` for the Global
/// orchestrator chat (no project-scoped commands to read).
#[tauri::command]
pub async fn list_claude_code_slash_commands(
    project_root: Option<String>,
) -> Result<Vec<ClaudeSlashCommand>, String> {
    // Use a name-keyed map so project-scoped commands override user-global
    // ones (matches Claude Code's own override precedence). Builtins are
    // overridden by either — that way a user can shadow `/help` with their
    // own version if they really want.
    let mut by_name: BTreeMap<String, ClaudeSlashCommand> = BTreeMap::new();

    for (name, desc) in BUILTIN_COMMANDS {
        by_name.insert(
            (*name).to_string(),
            ClaudeSlashCommand {
                name: (*name).to_string(),
                description: (*desc).to_string(),
                source: ClaudeSlashSource::Builtin,
            },
        );
    }

    if let Some(home) = rustic_agent::skills::home_dir() {
        let user_dir = home.join(".claude").join("commands");
        for cmd in scan_dir(&user_dir, ClaudeSlashSource::User) {
            by_name.insert(cmd.name.clone(), cmd);
        }
    }

    if let Some(root) = project_root.as_deref() {
        let project_dir = PathBuf::from(root).join(".claude").join("commands");
        for cmd in scan_dir(&project_dir, ClaudeSlashSource::Project) {
            by_name.insert(cmd.name.clone(), cmd);
        }
    }

    Ok(by_name.into_values().collect())
}

fn scan_dir(dir: &Path, source: ClaudeSlashSource) -> Vec<ClaudeSlashCommand> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        let description = std::fs::read_to_string(&path)
            .ok()
            .as_deref()
            .map(extract_description)
            .unwrap_or_default();
        out.push(ClaudeSlashCommand {
            name: stem.to_string(),
            description,
            source: source.clone(),
        });
    }
    out
}

/// Pull a one-line description out of a slash-command markdown file. We try
/// (in order) the H1 heading text and the first non-blank, non-frontmatter
/// line. If neither exists, return an empty string — better blank than wrong.
fn extract_description(body: &str) -> String {
    let mut lines = body.lines();

    // Skip a YAML-style frontmatter block if present.
    if let Some(first) = lines.clone().next() {
        if first.trim() == "---" {
            let mut after_open = false;
            let mut consumed = 0;
            for line in body.lines() {
                consumed += 1;
                if !after_open {
                    after_open = line.trim() == "---";
                    continue;
                }
                if line.trim() == "---" {
                    break;
                }
            }
            // Re-build the iterator past the closing `---`.
            lines = body.lines();
            for _ in 0..consumed {
                lines.next();
            }
        }
    }

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            return rest.trim().to_string();
        }
        // Skip leftover frontmatter markers in malformed files.
        if line.starts_with("---") {
            continue;
        }
        // Limit to a single line; collapse trailing whitespace.
        let trimmed = line.trim();
        // Keep at most ~120 chars so the picker subtitle stays one line.
        if trimmed.chars().count() > 120 {
            return trimmed.chars().take(117).collect::<String>() + "…";
        }
        return trimmed.to_string();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_h1() {
        let body = "# Run all tests\n\nDetails go here.";
        assert_eq!(extract_description(body), "Run all tests");
    }

    #[test]
    fn extract_first_line_when_no_h1() {
        let body = "Run the test suite.\n\nMore prose.";
        assert_eq!(extract_description(body), "Run the test suite.");
    }

    #[test]
    fn skip_frontmatter() {
        let body = "---\nname: tests\n---\n# Run all tests";
        assert_eq!(extract_description(body), "Run all tests");
    }

    #[test]
    fn empty_for_blank_file() {
        assert_eq!(extract_description(""), "");
        assert_eq!(extract_description("\n\n\n"), "");
    }

    #[test]
    fn truncate_long_first_line() {
        let body = "x".repeat(200);
        let out = extract_description(&body);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 118);
    }
}
