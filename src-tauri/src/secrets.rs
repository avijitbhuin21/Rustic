//! Thin wrapper over the OS keychain for storing per-provider API keys and
//! other long-lived secrets. Falls back gracefully (returns None / Ok(())) if
//! the OS does not have a working keyring service so the app stays usable on
//! headless / locked-down environments — the agent will simply prompt for the
//! key again.
//!
//! On macOS every secret lives inside ONE keychain item (a JSON map) so the
//! user gets a single access prompt per launch instead of one per secret —
//! unsigned/ad-hoc builds re-prompt after every update, which multiplied badly.
//! Legacy one-item-per-secret entries are migrated lazily on first read.
//! Windows/Linux keep one item per secret: they don't prompt, and Windows
//! Credential Manager caps a blob at ~2.5 KB, which a combined map can exceed.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

const SERVICE: &str = "com.rustic.editor";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const VAULT_ACCOUNT: &str = "rustic-vault";

trait ItemBackend {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// One keychain item per account, via the `keyring` crate.
struct KeyringItems;

fn entry_for(account: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).ok()
}

impl ItemBackend for KeyringItems {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let entry = entry_for(account).ok_or_else(|| "keyring unavailable".to_string())?;
        entry
            .set_password(secret)
            .map_err(|e| format!("keyring set failed: {}", e))
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
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

    fn delete(&self, account: &str) -> Result<(), String> {
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
}

enum Load {
    Unloaded,
    Ready(HashMap<String, String>),
    // Sticky for the session: retrying after a Deny would re-prompt on every call.
    Failed(String),
}

struct VaultState {
    load: Load,
    /// Accounts known to have no legacy per-account item left.
    legacy_clear: HashSet<String>,
}

/// All secrets in a single backend item, cached in memory after one read.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Vault<B: ItemBackend> {
    items: B,
    state: Mutex<VaultState>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl<B: ItemBackend> Vault<B> {
    /// Create an unloaded vault; the backend is not touched until first use.
    fn new(items: B) -> Self {
        Self {
            items,
            state: Mutex::new(VaultState {
                load: Load::Unloaded,
                legacy_clear: HashSet::new(),
            }),
        }
    }

    /// Lock the state, reading the vault item on first access.
    fn state(&self) -> MutexGuard<'_, VaultState> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(st.load, Load::Unloaded) {
            st.load = match self.items.get(VAULT_ACCOUNT) {
                Ok(Some(json)) => match serde_json::from_str(&json) {
                    Ok(map) => Load::Ready(map),
                    Err(e) => {
                        tracing::error!(error = %e, "[secrets] keychain vault unreadable; starting empty");
                        Load::Ready(HashMap::new())
                    }
                },
                Ok(None) => Load::Ready(HashMap::new()),
                Err(e) => {
                    tracing::error!(error = %e, "[secrets] keychain vault access failed");
                    Load::Failed(format!(
                        "keychain access denied or unavailable ({}); restart Rustic to retry",
                        e
                    ))
                }
            };
        }
        st
    }

    fn persist(&self, map: &HashMap<String, String>) -> Result<(), String> {
        let json = serde_json::to_string(map).map_err(|e| e.to_string())?;
        self.items.set(VAULT_ACCOUNT, &json)
    }

    /// Best-effort removal of a legacy per-account item so it can't resurrect later.
    fn clear_legacy(&self, account: &str, legacy_clear: &mut HashSet<String>) {
        if legacy_clear.contains(account) {
            return;
        }
        match self.items.delete(account) {
            Ok(()) => {
                legacy_clear.insert(account.to_string());
            }
            Err(e) => {
                tracing::warn!(account = %account, error = %e, "[secrets] legacy keychain item not removed")
            }
        }
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let mut st = self.state();
        let VaultState { load, legacy_clear } = &mut *st;
        let map = ready(load)?;
        if let Some(v) = map.get(account) {
            return Ok(Some(v.clone()));
        }
        if legacy_clear.contains(account) {
            return Ok(None);
        }
        match self.items.get(account)? {
            None => {
                legacy_clear.insert(account.to_string());
                Ok(None)
            }
            Some(secret) => {
                let mut next = map.clone();
                next.insert(account.to_string(), secret.clone());
                match self.persist(&next) {
                    Ok(()) => {
                        *map = next;
                        tracing::info!(account = %account, "[secrets] migrated legacy keychain item into vault");
                        self.clear_legacy(account, legacy_clear);
                    }
                    Err(e) => {
                        tracing::warn!(account = %account, error = %e, "[secrets] vault migration write failed; legacy item kept")
                    }
                }
                Ok(Some(secret))
            }
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let mut st = self.state();
        let VaultState { load, legacy_clear } = &mut *st;
        let map = ready(load)?;
        let mut next = map.clone();
        next.insert(account.to_string(), secret.to_string());
        self.persist(&next)?;
        *map = next;
        self.clear_legacy(account, legacy_clear);
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let mut st = self.state();
        let VaultState { load, legacy_clear } = &mut *st;
        let map = ready(load)?;
        if map.contains_key(account) {
            let mut next = map.clone();
            next.remove(account);
            self.persist(&next)?;
            *map = next;
        }
        self.clear_legacy(account, legacy_clear);
        Ok(())
    }
}

fn ready(load: &mut Load) -> Result<&mut HashMap<String, String>, String> {
    match load {
        Load::Ready(map) => Ok(map),
        Load::Failed(e) => Err(e.clone()),
        Load::Unloaded => Err("keychain vault not loaded".to_string()),
    }
}

#[cfg(target_os = "macos")]
fn vault() -> &'static Vault<KeyringItems> {
    static VAULT: std::sync::OnceLock<Vault<KeyringItems>> = std::sync::OnceLock::new();
    VAULT.get_or_init(|| Vault::new(KeyringItems))
}

/// Store a secret. Returns Err(reason) on failure so callers can surface
/// to the user; callers that want best-effort can map_err and ignore.
pub fn set(account: &str, secret: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return vault().set(account, secret);
    #[cfg(not(target_os = "macos"))]
    return KeyringItems.set(account, secret);
}

/// Retrieve a secret. Returns None if not present (NotFound) or the OS keyring
/// is unavailable; surfaces other errors as Err.
pub fn get(account: &str) -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    return vault().get(account);
    #[cfg(not(target_os = "macos"))]
    return KeyringItems.get(account);
}

/// Delete a secret. NotFound is treated as success since deleting a missing
/// entry is idempotent from the caller's perspective.
pub fn delete(account: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return vault().delete(account);
    #[cfg(not(target_os = "macos"))]
    return KeyringItems.delete(account);
}

/// Account name for a provider's API key. Re-exported from `rustic-app` so
/// both transports derive the identical account string (a drifted copy here
/// would silently split desktop and server keychains).
pub use rustic_app::secrets::provider_account;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// In-memory keychain that counts reads per account and can refuse access.
    #[derive(Default)]
    struct FakeItems {
        items: RefCell<HashMap<String, String>>,
        reads: RefCell<HashMap<String, usize>>,
        deny: bool,
    }

    impl ItemBackend for FakeItems {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            *self
                .reads
                .borrow_mut()
                .entry(account.to_string())
                .or_default() += 1;
            if self.deny {
                return Err("user denied".into());
            }
            Ok(self.items.borrow().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            self.items
                .borrow_mut()
                .insert(account.into(), secret.into());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.items.borrow_mut().remove(account);
            Ok(())
        }
    }

    fn reads(v: &Vault<FakeItems>, account: &str) -> usize {
        v.items.reads.borrow().get(account).copied().unwrap_or(0)
    }

    #[test]
    fn vault_read_once_and_roundtrips() {
        let v = Vault::new(FakeItems::default());
        v.set("provider:openai", "sk-1").unwrap();
        v.set("git-token", "ghp").unwrap();
        assert_eq!(v.get("provider:openai").unwrap().as_deref(), Some("sk-1"));
        assert_eq!(v.get("git-token").unwrap().as_deref(), Some("ghp"));
        assert_eq!(reads(&v, VAULT_ACCOUNT), 1);
        v.delete("provider:openai").unwrap();
        assert_eq!(v.get("provider:openai").unwrap(), None);

        let items = v.items;
        let reopened = Vault::new(items);
        assert_eq!(reopened.get("git-token").unwrap().as_deref(), Some("ghp"));
        assert_eq!(reopened.get("provider:openai").unwrap(), None);
    }

    #[test]
    fn legacy_items_migrate_into_vault() {
        let fake = FakeItems::default();
        fake.items
            .borrow_mut()
            .insert("provider:claude".into(), "sk-old".into());
        let v = Vault::new(fake);
        assert_eq!(v.get("provider:claude").unwrap().as_deref(), Some("sk-old"));
        assert!(!v.items.items.borrow().contains_key("provider:claude"));
        assert_eq!(v.get("provider:claude").unwrap().as_deref(), Some("sk-old"));
        assert_eq!(reads(&v, "provider:claude"), 1);

        assert_eq!(v.get("provider:none").unwrap(), None);
        assert_eq!(v.get("provider:none").unwrap(), None);
        assert_eq!(reads(&v, "provider:none"), 1);
    }

    #[test]
    fn delete_removes_stale_legacy_item() {
        let fake = FakeItems::default();
        fake.items
            .borrow_mut()
            .insert("git-token".into(), "old".into());
        let v = Vault::new(fake);
        v.delete("git-token").unwrap();
        let reopened = Vault::new(v.items);
        assert_eq!(reopened.get("git-token").unwrap(), None);
    }

    #[test]
    fn denied_access_is_sticky_and_never_overwrites() {
        let fake = FakeItems {
            deny: true,
            ..Default::default()
        };
        let v = Vault::new(fake);
        assert!(v.get("a").is_err());
        assert!(v.get("b").is_err());
        assert!(v.set("a", "x").is_err());
        assert_eq!(reads(&v, VAULT_ACCOUNT), 1);
        assert!(v.items.items.borrow().is_empty());
    }
}
