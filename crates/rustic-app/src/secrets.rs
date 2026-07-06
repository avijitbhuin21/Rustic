//! Pluggable secret storage.
//!
//! The desktop shell stores per-provider API keys and the git token in the OS
//! keychain (`keyring`), which does not exist on a headless Linux server. This
//! module defines the [`SecretStore`] trait both transports program against,
//! plus two server-side implementations:
//!
//! * [`FileSecretStore`] — a JSON file under the data dir, the writable default
//!   for the server. On Unix the file is created `0600` (owner-only). NOTE:
//!   values are stored in cleartext inside that file; the protection is
//!   filesystem permissions + the fact the box itself is the trust boundary.
//!   For at-rest encryption, layer a disk-encrypted volume or a secret manager.
//! * [`EnvSecretStore`] — read-only, resolves `account` to the environment
//!   variable `RUSTIC_SECRET_<SANITIZED_ACCOUNT>`. Useful for injecting keys via
//!   container orchestration without writing them to disk. Falls through to a
//!   wrapped writable store for `set`/`delete`.
//!
//! The keychain implementation lives in `src-tauri` (it owns the `keyring`
//! dependency) and implements this same trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Abstract secret backend. All methods mirror the historical free functions in
/// `src-tauri/src/secrets.rs` so call sites translate 1:1.
pub trait SecretStore: Send + Sync {
    /// Store a secret. `Err` carries a human-readable reason for surfacing.
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    /// Retrieve a secret. `Ok(None)` means "not configured" (distinct from a
    /// backend error, which is `Err`).
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    /// Delete a secret. Deleting a missing entry is success (idempotent).
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// Stable account name for a provider's API key. Identical to the desktop
/// helper so the same logical key resolves across both transports.
pub fn provider_account(provider_type: &str, instance_name: Option<&str>) -> String {
    match instance_name.filter(|s| !s.trim().is_empty()) {
        Some(name) => format!("provider:{}:{}", provider_type, name),
        None => format!("provider:{}", provider_type),
    }
}

/// File-backed secret store: a single JSON object `{account: secret}` persisted
/// atomically. Thread-safe via an internal mutex. The file is read once when
/// the store is opened and the in-memory cache is authoritative afterwards
/// (every write persists the full map atomically); external edits to the file
/// while the process is running are NOT picked up until restart, so don't
/// point two live processes at the same secrets file.
pub struct FileSecretStore {
    path: PathBuf,
    cache: Mutex<HashMap<String, String>>,
}

impl FileSecretStore {
    /// Open (or lazily create) the secret file at `<data_dir>/secrets.json`.
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("secrets.json");
        let cache = Self::load(&path).unwrap_or_default();
        Self {
            path,
            cache: Mutex::new(cache),
        }
    }

    fn load(path: &Path) -> Option<HashMap<String, String>> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn persist(&self, map: &HashMap<String, String>) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Write to a temp sibling then rename for atomicity.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        Self::lock_down_perms(&tmp);
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Self::lock_down_perms(&self.path);
        Ok(())
    }

    #[cfg(unix)]
    fn lock_down_perms(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }

    /// Windows equivalent of the unix `0600` chmod: strip inherited ACEs and
    /// grant full control to the current user only, via `icacls`. Best-effort
    /// (like the unix branch) — failures are logged, never fatal, so a locked-
    /// down or icacls-less environment still gets a working (if
    /// default-ACL'd) secrets file.
    #[cfg(windows)]
    fn lock_down_perms(path: &Path) {
        let user = match std::env::var("USERNAME") {
            Ok(u) if !u.trim().is_empty() => u,
            _ => {
                tracing::warn!(
                    "cannot restrict ACL on {}: USERNAME env var unavailable",
                    path.display()
                );
                return;
            }
        };
        let mut cmd = std::process::Command::new("icacls");
        cmd.arg(path)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{}:F", user));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match cmd.output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => tracing::warn!(
                "icacls failed to restrict ACL on {} (exit {}): {}",
                path.display(),
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => tracing::warn!(
                "failed to spawn icacls to restrict ACL on {}: {}",
                path.display(),
                e
            ),
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn lock_down_perms(_path: &Path) {}
}

impl SecretStore for FileSecretStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let mut cache = self.cache.lock().map_err(|e| e.to_string())?;
        cache.insert(account.to_string(), secret.to_string());
        self.persist(&cache)
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let cache = self.cache.lock().map_err(|e| e.to_string())?;
        Ok(cache.get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let mut cache = self.cache.lock().map_err(|e| e.to_string())?;
        if cache.remove(account).is_some() {
            self.persist(&cache)?;
        }
        Ok(())
    }
}

/// Environment-variable secret store. `get` reads
/// `RUSTIC_SECRET_<SANITIZED_ACCOUNT>` where the account is upper-cased and any
/// non-alphanumeric char becomes `_`. Writes fall through to an inner store so
/// keys provided via the UI still persist somewhere writable.
pub struct EnvSecretStore<S: SecretStore> {
    inner: S,
}

impl<S: SecretStore> EnvSecretStore<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    fn env_key(account: &str) -> String {
        let sanitized: String = account
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        format!("RUSTIC_SECRET_{}", sanitized)
    }
}

impl<S: SecretStore> SecretStore for EnvSecretStore<S> {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.inner.set(account, secret)
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        if let Ok(v) = std::env::var(Self::env_key(account)) {
            if !v.is_empty() {
                return Ok(Some(v));
            }
        }
        self.inner.get(account)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.inner.delete(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate the process environment (`set_var` /
    /// `remove_var` are process-global, so parallel test threads would race).
    /// Any test touching `std::env::set_var` must hold this lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn file_store_roundtrips() {
        let dir = std::env::temp_dir().join(format!("rustic-secret-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = FileSecretStore::new(&dir);
        assert_eq!(store.get("provider:openai").unwrap(), None);
        store.set("provider:openai", "sk-123").unwrap();
        assert_eq!(
            store.get("provider:openai").unwrap().as_deref(),
            Some("sk-123")
        );
        // A fresh store reading the same file sees the persisted value.
        let store2 = FileSecretStore::new(&dir);
        assert_eq!(
            store2.get("provider:openai").unwrap().as_deref(),
            Some("sk-123")
        );
        store.delete("provider:openai").unwrap();
        assert_eq!(store.get("provider:openai").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_store_prefers_env() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("rustic-env-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inner = FileSecretStore::new(&dir);
        inner.set("provider:anthropic", "file-key").unwrap();
        let store = EnvSecretStore::new(inner);
        std::env::set_var("RUSTIC_SECRET_PROVIDER_ANTHROPIC", "env-key");
        assert_eq!(
            store.get("provider:anthropic").unwrap().as_deref(),
            Some("env-key")
        );
        std::env::remove_var("RUSTIC_SECRET_PROVIDER_ANTHROPIC");
        assert_eq!(
            store.get("provider:anthropic").unwrap().as_deref(),
            Some("file-key")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
