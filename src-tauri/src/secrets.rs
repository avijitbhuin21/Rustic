//! Thin wrapper over the OS keychain for storing per-provider API keys and
//! other long-lived secrets. Falls back gracefully (returns None / Ok(())) if
//! the OS does not have a working keyring service so the app stays usable on
//! headless / locked-down environments — the agent will simply prompt for the
//! key again.

const SERVICE: &str = "com.rustic.editor";

fn entry_for(account: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).ok()
}

/// Store a secret. Returns Err(reason) on failure so callers can surface
/// to the user; callers that want best-effort can map_err and ignore.
pub fn set(account: &str, secret: &str) -> Result<(), String> {
    let entry = entry_for(account).ok_or_else(|| "keyring unavailable".to_string())?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring set failed: {}", e))
}

/// Retrieve a secret. Returns None if not present (NotFound) or the OS keyring
/// is unavailable; surfaces other errors as Err.
pub fn get(account: &str) -> Result<Option<String>, String> {
    let entry = match entry_for(account) {
        Some(e) => e,
        None => return Ok(None),
    };
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring get failed: {}", e)),
    }
}

/// Delete a secret. NotFound is treated as success since deleting a missing
/// entry is idempotent from the caller's perspective.
pub fn delete(account: &str) -> Result<(), String> {
    let entry = match entry_for(account) {
        Some(e) => e,
        None => return Ok(()),
    };
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete failed: {}", e)),
    }
}

/// Account name for a provider's API key. Use a stable, unique string so the
/// same key survives provider renames in the UI.
pub fn provider_account(provider_type: &str, instance_name: Option<&str>) -> String {
    match instance_name.filter(|s| !s.trim().is_empty()) {
        Some(name) => format!("provider:{}:{}", provider_type, name),
        None => format!("provider:{}", provider_type),
    }
}
