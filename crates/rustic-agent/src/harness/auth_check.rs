//! Detect installed harness binaries and whether the user has signed in.
//!
//! We never read auth tokens — just check that the CLI's auth-state file
//! exists and is non-empty. If a user wants to switch accounts they do it
//! at the CLI level (`claude` interactive, `codex login`); Rustic stays out
//! of that flow (plan §13.3).
//!
//! Used by:
//! * Onboarding wizard — to render the per-provider status row.
//! * Settings → Subscriptions panel — to validate a custom binary path the
//!   user typed in.
//! * Task launch — to surface a clear `harness_not_authenticated` error
//!   instead of letting the CLI spawn and immediately fail.

use crate::harness::process_spawn::{HarnessSpawnSpec, SpawnedHarnessChild};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::timeout;

/// Result of a single auth-state check. Distinct from the `Harness` enum so
/// the frontend can render `Installed, not signed in` vs `Not installed` vs
/// `Installed & authenticated` without re-running the probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HarnessAuthStatus {
    /// Binary not on PATH and the user-supplied override (if any) didn't resolve.
    NotInstalled { reason: String },
    /// Binary works but the per-CLI auth-state file is missing or empty.
    NotAuthenticated { version: Option<String> },
    /// Binary works and auth-state file looks plausible. We do not validate
    /// that the token is unexpired — only the CLI itself can do that, and
    /// surfacing the failure once they try to use it is the cheapest path.
    Authenticated { version: Option<String> },
    /// Binary detection itself errored — e.g. the spawn timed out, or
    /// `--version` exited non-zero. Surface stderr so the settings panel
    /// can show it inline.
    ProbeFailed { detail: String },
}

/// One probe per harness kind. Cheap; safe to run in parallel.
pub async fn probe_claude_code(binary_override: Option<&Path>) -> HarnessAuthStatus {
    let program = binary_override
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "claude".to_string());
    probe_with_version(program, claude_auth_path()).await
}

pub async fn probe_codex(binary_override: Option<&Path>) -> HarnessAuthStatus {
    let program = binary_override
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "codex".to_string());
    probe_with_version(program, codex_auth_dir_marker()).await
}

/// Shared probe: run `<binary> --version`, then check the auth-state file.
async fn probe_with_version(program: String, auth_marker: Option<PathBuf>) -> HarnessAuthStatus {
    let version_result = run_version_probe(&program).await;
    match version_result {
        Err(e) => HarnessAuthStatus::NotInstalled {
            reason: e.to_string(),
        },
        Ok(VersionProbe::Failed { detail }) => HarnessAuthStatus::ProbeFailed { detail },
        Ok(VersionProbe::Ok { version }) => {
            if has_auth_marker(auth_marker.as_deref()) {
                HarnessAuthStatus::Authenticated { version }
            } else {
                HarnessAuthStatus::NotAuthenticated { version }
            }
        }
    }
}

enum VersionProbe {
    Ok { version: Option<String> },
    Failed { detail: String },
}

/// Spawn `<program> --version`, read up to one line of stdout, kill if it
/// hangs. Returns the trimmed first line, or whatever stderr says on failure.
async fn run_version_probe(program: &str) -> Result<VersionProbe> {
    let spec = HarnessSpawnSpec {
        program: program.to_string(),
        args: vec!["--version".to_string()],
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        env: vec![],
    };

    let mut child = SpawnedHarnessChild::spawn(spec)?;

    let mut stdout = child
        .stdout
        .take()
        .expect("stdout was piped at spawn time");
    let mut stderr = child
        .stderr
        .take()
        .expect("stderr was piped at spawn time");

    let read_stdout = async {
        let mut lines = BufReader::new(&mut stdout).lines();
        lines.next_line().await.ok().flatten()
    };

    let first_line = match timeout(Duration::from_secs(5), read_stdout).await {
        Ok(line) => line,
        Err(_) => {
            let _ = child.kill().await;
            return Ok(VersionProbe::Failed {
                detail: "version probe timed out after 5s".to_string(),
            });
        }
    };

    // Drain stderr non-blockingly in case the CLI complains.
    let mut stderr_buf = String::new();
    let read_stderr = async {
        use tokio::io::AsyncReadExt;
        let _ = stderr.read_to_string(&mut stderr_buf).await;
    };
    let _ = timeout(Duration::from_millis(500), read_stderr).await;

    let _ = child.kill().await;

    match first_line {
        Some(line) if !line.trim().is_empty() => Ok(VersionProbe::Ok {
            version: Some(line.trim().to_string()),
        }),
        _ if !stderr_buf.trim().is_empty() => Ok(VersionProbe::Failed {
            detail: stderr_buf.trim().to_string(),
        }),
        _ => Ok(VersionProbe::Ok { version: None }),
    }
}

fn has_auth_marker(p: Option<&Path>) -> bool {
    let Some(p) = p else { return false };
    match std::fs::metadata(p) {
        Ok(meta) if meta.is_file() => meta.len() > 0,
        Ok(meta) if meta.is_dir() => {
            // Treat a non-empty dir as "probably authenticated".
            std::fs::read_dir(p)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// `~/.claude/.credentials.json` is what Claude Code writes after `claude`
/// interactive login. The exact filename has shifted across versions
/// (`credentials.json` and `.credentials.json` have both been used); we
/// accept either, falling back to the directory presence as a last resort.
fn claude_auth_path() -> Option<PathBuf> {
    let home = crate::skills::home_dir()?;
    let dir = home.join(".claude");
    for candidate in [".credentials.json", "credentials.json"] {
        let p = dir.join(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

/// Codex stores its auth state under `~/.codex/auth.json` (current versions)
/// or just `~/.codex/`. Same pattern as above.
fn codex_auth_dir_marker() -> Option<PathBuf> {
    let home = crate::skills::home_dir()?;
    let dir = home.join(".codex");
    let candidate = dir.join("auth.json");
    if candidate.exists() {
        return Some(candidate);
    }
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_marker_handles_missing_path() {
        assert!(!has_auth_marker(None));
        assert!(!has_auth_marker(Some(Path::new(
            "/nonexistent/path/that/should/not/exist/anywhere"
        ))));
    }

    #[tokio::test]
    async fn probe_missing_binary_returns_not_installed() {
        let status =
            probe_with_version("definitely-not-a-real-binary-xyz".to_string(), None).await;
        match status {
            HarnessAuthStatus::NotInstalled { .. } | HarnessAuthStatus::ProbeFailed { .. } => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }
}
