//! Agent self-extension support: install / uninstall of skills, workflows,
//! and MCP servers initiated by the agent itself.
//!
//! Safety model (agreed with the user):
//! - Tier 1 (self-authored, project scope): no blocking consent — the tool
//!   call is visible in chat and an audit entry is written.
//! - Tier 2 (fetched from a URL, or installed to global scope): explicit
//!   user consent via the permission broker, always, in every mode.
//! - MCP servers: always require consent (stdio = arbitrary command
//!   execution; remote = live exfiltration channel).
//! - Uninstalls never destroy data: files move to `~/.rustic/trash/` and MCP
//!   configs are backed up there before removal.
//! - Everything is appended to `~/.rustic/extensions-audit.jsonl`.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Sidecar file recording where an installed extension came from.
pub const PROVENANCE_FILE: &str = ".provenance.json";

/// Hard cap on fetched extension content (matches SKILL_MAX_BYTES).
pub const FETCH_MAX_BYTES: usize = 1024 * 1024;

/// Preview cap for the consent dialog — enough to audit a real skill without
/// rendering a megabyte into the dialog. The SHA-256 in the header covers the
/// full content regardless.
pub const CONSENT_PREVIEW_MAX_CHARS: usize = 6000;

/// Records the origin of an agent-installed extension. Written next to the
/// installed artifact so trust decisions survive restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// Always "agent" for installs performed via extension tools.
    pub origin: String,
    /// "inline" for self-authored content, otherwise the source URL.
    pub source: String,
    /// SHA-256 (lowercase hex) of the installed content.
    pub sha256: String,
    /// RFC 3339 install timestamp.
    pub installed_at: String,
    /// Task that performed the install.
    pub task_id: String,
}

impl Provenance {
    /// True when the content was pulled from outside the conversation (URL).
    pub fn is_external(&self) -> bool {
        self.source != "inline"
    }
}

/// Validate an extension name for safe use as a single path component.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(anyhow!("name must be 1-64 characters"));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(anyhow!(
            "name must start with a lowercase letter or digit (got `{}`)",
            name
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(anyhow!(
            "name must be kebab-case: lowercase letters, digits, `-` or `_` only (got `{}`)",
            name
        ));
    }
    Ok(())
}

/// `~/.rustic` — shared home for global extensions, trash, and the audit log.
pub fn rustic_home() -> Option<PathBuf> {
    crate::skills::home_dir().map(|h| h.join(".rustic"))
}

/// Trash directory used for reversible uninstalls: `~/.rustic/trash/`.
pub fn trash_dir() -> Option<PathBuf> {
    rustic_home().map(|h| h.join("trash"))
}

/// Path of the append-only audit log.
pub fn audit_path() -> Option<PathBuf> {
    rustic_home().map(|h| h.join("extensions-audit.jsonl"))
}

/// One line in `extensions-audit.jsonl`.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub at: String,
    /// "install" | "uninstall"
    pub action: String,
    /// "skill" | "workflow" | "mcp_server"
    pub kind: String,
    pub name: String,
    pub scope: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Best-effort append to the audit log; never fails the install itself.
pub fn audit(entry: &AuditEntry) {
    let Some(path) = audit_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut line) = serde_json::to_string(entry) else {
        return;
    };
    line.push('\n');
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Build a ready-to-write audit entry stamped with the current time.
pub fn audit_entry(
    action: &str,
    kind: &str,
    name: &str,
    scope: &str,
    source: &str,
    sha256: Option<String>,
    task_id: &str,
    detail: Option<String>,
) -> AuditEntry {
    AuditEntry {
        at: chrono::Utc::now().to_rfc3339(),
        action: action.to_string(),
        kind: kind.to_string(),
        name: name.to_string(),
        scope: scope.to_string(),
        source: source.to_string(),
        sha256,
        task_id: task_id.to_string(),
        detail,
    }
}

/// Read the provenance sidecar inside a skill directory (or next to a file).
pub fn read_provenance(dir: &Path) -> Option<Provenance> {
    let p = dir.join(PROVENANCE_FILE);
    let text = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write a provenance sidecar into `dir`.
pub fn write_provenance(dir: &Path, prov: &Provenance) -> Result<()> {
    let text = serde_json::to_string_pretty(prov)?;
    std::fs::write(dir.join(PROVENANCE_FILE), text)?;
    Ok(())
}

/// Sidecar path for a standalone workflow file: `<file>.provenance.json`.
pub fn workflow_provenance_path(md_path: &Path) -> PathBuf {
    let mut s = md_path.as_os_str().to_os_string();
    s.push(".provenance.json");
    PathBuf::from(s)
}

/// Move a file or directory into the trash; returns the destination path.
pub fn move_to_trash(src: &Path) -> Result<PathBuf> {
    let dir = trash_dir().ok_or_else(|| anyhow!("cannot resolve home directory for trash"))?;
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid source path: {}", src.display()))?;
    let dest = dir.join(format!("{}-{}", stamp, file_name));
    if std::fs::rename(src, &dest).is_ok() {
        return Ok(dest);
    }
    // Cross-volume fallback (e.g. project on D:, home on C:): copy then delete.
    if src.is_dir() {
        copy_dir_recursive(src, &dest)?;
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::copy(src, &dest)?;
        std::fs::remove_file(src)?;
    }
    Ok(dest)
}

/// Recursively copy `src` directory into `dst` (created fresh).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Fetch UTF-8 text from an http(s) URL with a 30s timeout and a 1 MiB cap.
pub async fn fetch_text(url: &str) -> Result<String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(anyhow!("only http(s) URLs are supported (got `{}`)", url));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("fetch failed: HTTP {} from {}", status, url));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() > FETCH_MAX_BYTES {
        return Err(anyhow!(
            "content too large: {} bytes (cap {})",
            bytes.len(),
            FETCH_MAX_BYTES
        ));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| anyhow!("content is not valid UTF-8 text"))
}

/// Cap a string for the consent preview, appending a truncation marker.
pub fn preview_capped(content: &str) -> String {
    if content.chars().count() <= CONSENT_PREVIEW_MAX_CHARS {
        return content.to_string();
    }
    let capped: String = content.chars().take(CONSENT_PREVIEW_MAX_CHARS).collect();
    format!(
        "{}\n\n… [truncated for preview — SHA-256 above covers the FULL content]",
        capped
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(validate_name("my-skill").is_ok());
        assert!(validate_name("skill_2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("-lead").is_err());
        assert!(validate_name("Upper").is_err());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name("a b").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn provenance_external_detection() {
        let p = Provenance {
            origin: "agent".into(),
            source: "inline".into(),
            sha256: "abc".into(),
            installed_at: "now".into(),
            task_id: "t".into(),
        };
        assert!(!p.is_external());
        let q = Provenance {
            source: "https://example.com/s.md".into(),
            ..p
        };
        assert!(q.is_external());
    }

    #[test]
    fn workflow_sidecar_path_appends_suffix() {
        let p = workflow_provenance_path(Path::new("/x/flow.md"));
        assert!(p.to_string_lossy().ends_with("flow.md.provenance.json"));
    }
}
