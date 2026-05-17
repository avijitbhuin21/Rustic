//! Slash-command discovery for Claude Code's chat-input autocomplete.
//! Project-scoped commands override user-global; stream-json doesn't process
//! slash commands (REPL-only), so the host inlines the body on expansion.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSlashCommand {
    pub name: String,
    pub description: String,
    pub source: ClaudeSlashSource,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeSlashSource {
    Builtin,
    User,
    Project,
}

/// Update when the CLI gains new builtin commands.
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

#[tauri::command]
pub async fn list_claude_code_slash_commands(
    project_root: Option<String>,
) -> Result<Vec<ClaudeSlashCommand>, String> {
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

#[tauri::command]
pub async fn get_claude_code_slash_command_body(
    project_root: Option<String>,
    name: String,
) -> Result<Option<String>, String> {
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return Ok(None);
    }
    let file_name = format!("{}.md", name);

    if let Some(root) = project_root.as_deref() {
        let path = PathBuf::from(root)
            .join(".claude")
            .join("commands")
            .join(&file_name);
        if let Some(body) = read_capped_md(&path) {
            return Ok(Some(strip_frontmatter(&body).trim().to_string()));
        }
    }

    if let Some(home) = rustic_agent::skills::home_dir() {
        let path = home.join(".claude").join("commands").join(&file_name);
        if let Some(body) = read_capped_md(&path) {
            return Ok(Some(strip_frontmatter(&body).trim().to_string()));
        }
    }

    Ok(None)
}

/// F-23: 256 KiB cap — prevents DoS via oversized prompt-template files.
fn read_capped_md(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    const MAX: u64 = 256 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = String::new();
    f.by_ref().take(MAX).read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// Strip a leading `---`-delimited YAML frontmatter block, if present. The
/// frontmatter is metadata for the picker (description, etc.) — it should
/// not be sent to the model as part of the prompt.
fn strip_frontmatter(body: &str) -> &str {
    let mut chars = body.char_indices();
    let trimmed_start = chars
        .by_ref()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(body.len());
    let rest = &body[trimmed_start..];
    if !rest.starts_with("---") {
        return body;
    }
    // Find the closing `---` on its own line.
    let after_open = &rest[3..];
    let mut search_pos = 0usize;
    while let Some(idx) = after_open[search_pos..].find("\n---") {
        let abs = search_pos + idx + 4; // past "\n---"
        let tail = &after_open[abs..];
        if tail.starts_with('\n') || tail.is_empty() || tail.starts_with("\r\n") {
            return &after_open[abs..];
        }
        search_pos = abs;
    }
    body
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

    #[test]
    fn strip_frontmatter_removes_yaml_block() {
        let body = "---\nname: review\ndescription: code review\n---\nDo a code review of recent changes.";
        let stripped = strip_frontmatter(body).trim();
        assert_eq!(stripped, "Do a code review of recent changes.");
    }

    #[test]
    fn strip_frontmatter_preserves_body_without_frontmatter() {
        let body = "Do a code review of recent changes.";
        assert_eq!(strip_frontmatter(body), body);
    }

    #[test]
    fn strip_frontmatter_leaves_unterminated_block_alone() {
        // A leading `---` with no matching close shouldn't eat the whole file.
        let body = "---\nname: review\nDo a thing.";
        assert_eq!(strip_frontmatter(body), body);
    }
}
