//! Reads the FreeBuff (codebuff / manicode) CLI login from disk.
//!
//! The `freebuff` CLI (`npm i -g freebuff`, then `freebuff login`) stores its
//! account under `~/.config/manicode/credentials.json` on every platform —
//! including Windows, where `~` resolves to `%USERPROFILE%`. The file looks
//! like:
//!
//! ```json
//! { "default": { "authToken": "…", "name": "…", "email": "…",
//!                "fingerprintId": "…", "fingerprintHash": "…" } }
//! ```
//!
//! We only need `default.authToken` to talk to codebuff; the email is surfaced
//! in the Settings card so the user can confirm which account is logged in.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize)]
struct Account {
    #[serde(default, rename = "authToken")]
    auth_token: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CredentialsFile {
    #[serde(default)]
    default: Account,
}

/// Result of probing for a local FreeBuff login. Serialized straight to the
/// frontend by the `detect_freebuff` Tauri command.
#[derive(Debug, Clone, Serialize)]
pub struct DetectInfo {
    /// True when `credentials.json` exists, parses, and carries a non-empty token.
    pub available: bool,
    /// Account email (or name) when known — shown as "logged in as …".
    pub email: Option<String>,
    /// Human-readable reason when `available` is false (drives the Settings hint).
    pub reason: Option<String>,
}

/// `~/.config/manicode/credentials.json`. `~` is `%USERPROFILE%` on Windows,
/// `$HOME` elsewhere. Returns `None` only when neither home variable is set.
fn credentials_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|s| !s.is_empty()))?;
    let mut path = PathBuf::from(home);
    path.push(".config");
    path.push("manicode");
    path.push("credentials.json");
    Some(path)
}

fn load() -> Result<Account> {
    let path = credentials_path()
        .ok_or_else(|| anyhow!("could not resolve home directory (USERPROFILE / HOME unset)"))?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("FreeBuff is not logged in ({}): {}", path.display(), e))?;
    let parsed: CredentialsFile = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("credentials.json at {} is malformed: {}", path.display(), e))?;
    if parsed.default.auth_token.trim().is_empty() {
        return Err(anyhow!(
            "credentials.json has no authToken — run `freebuff login`"
        ));
    }
    Ok(parsed.default)
}

/// The bearer token sent to codebuff. Read fresh per request so a re-login is
/// picked up without restarting the app.
pub fn read_token() -> Result<String> {
    Ok(load()?.auth_token)
}

/// `(token, email)` for the currently logged-in CLI `default` account — used by
/// the settings token-pool UI to snapshot the active login into the pool.
pub fn read_account() -> Result<(String, Option<String>)> {
    let a = load()?;
    Ok((a.auth_token, a.email.or(a.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_account() {
        let raw = r#"{
            "default": {
                "id": "abc",
                "name": "avijit",
                "email": "a@example.com",
                "authToken": "tok-123",
                "fingerprintId": "fp",
                "fingerprintHash": "hash"
            }
        }"#;
        let parsed: CredentialsFile = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.default.auth_token, "tok-123");
        assert_eq!(parsed.default.email.as_deref(), Some("a@example.com"));
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let raw = r#"{ "default": { "authToken": "only-token" } }"#;
        let parsed: CredentialsFile = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.default.auth_token, "only-token");
        assert!(parsed.default.email.is_none());
    }
}

/// Non-failing probe for the Settings toggle.
pub fn detect() -> DetectInfo {
    match load() {
        Ok(acct) => DetectInfo {
            available: true,
            email: acct.email.or(acct.name),
            reason: None,
        },
        Err(e) => DetectInfo {
            available: false,
            email: None,
            reason: Some(e.to_string()),
        },
    }
}
