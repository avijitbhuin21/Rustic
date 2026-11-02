//! Deploy-time server configuration, read from the environment (a `.env` file
//! is loaded into the environment by the server binary before this runs).

use std::net::SocketAddr;
use std::path::PathBuf;

/// Resolved server configuration. Constructed via [`ServerConfig::from_env`].
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// The login password gating every route. Required — the server refuses to
    /// start without it (an unauthenticated dev box on a public port is the one
    /// thing we never want to ship).
    pub auth_password: String,
    /// Secret used to sign session tokens (HMAC). Derived from
    /// `RUSTIC_SESSION_SECRET` or, if absent, a random per-process value
    /// (which invalidates sessions across restarts — fine for single-user).
    pub session_secret: Vec<u8>,
    /// Address to bind, e.g. `0.0.0.0:8787`.
    pub bind_addr: SocketAddr,
    /// Application data directory (DB, logs, file-history, secrets file).
    pub data_dir: PathBuf,
    /// Directory containing the built web frontend (Vite `dist`) to serve.
    pub static_dir: PathBuf,
    /// Session lifetime in seconds.
    pub session_ttl_secs: u64,
    /// Max failed logins from one IP before a temporary lockout.
    pub login_max_attempts: u32,
    /// Lockout window in seconds after `login_max_attempts` failures.
    pub login_lockout_secs: u64,
    /// Wildcard preview domain for subdomain port-forwarding (e.g.
    /// `preview.example.com`, reached as `3000.preview.example.com`). `None`
    /// falls back to path-based `/proxy/<port>` forwarding.
    pub preview_domain: Option<String>,
    /// Parent domain to scope the session cookie to (e.g. `.example.com`) so it
    /// is shared with preview subdomains. Required for subdomain-mode auth.
    pub cookie_domain: Option<String>,
}

impl ServerConfig {
    /// Build config from environment variables. Returns a human-readable error
    /// describing the first missing/invalid required variable.
    pub fn from_env() -> Result<Self, String> {
        let auth_password = std::env::var("RUSTIC_AUTH_PASSWORD")
            .map_err(|_| "RUSTIC_AUTH_PASSWORD is required (the login password)".to_string())?;
        if auth_password.trim().is_empty() {
            return Err("RUSTIC_AUTH_PASSWORD must not be empty".to_string());
        }

        let session_secret = match std::env::var("RUSTIC_SESSION_SECRET") {
            Ok(s) if !s.is_empty() => s.into_bytes(),
            _ => {
                tracing::warn!(
                    "RUSTIC_SESSION_SECRET not set — using a random per-process secret; \
                     sessions will not survive a restart"
                );
                random_secret()
            }
        };

        let bind_str = match std::env::var("RUSTIC_BIND_ADDR") {
            Ok(s) if !s.is_empty() => s,
            _ => match std::env::var("PORT") {
                Ok(p) if !p.trim().is_empty() => format!("0.0.0.0:{}", p.trim()),
                _ => "0.0.0.0:8787".to_string(),
            },
        };
        let bind_addr: SocketAddr = bind_str
            .parse()
            .map_err(|e| format!("bind address '{}' is not valid: {}", bind_str, e))?;

        let data_dir = match std::env::var("RUSTIC_DATA_DIR") {
            Ok(d) if !d.is_empty() => PathBuf::from(d),
            _ => default_data_dir(),
        };

        let static_dir = match std::env::var("RUSTIC_STATIC_DIR") {
            Ok(d) if !d.is_empty() => PathBuf::from(d),
            _ => PathBuf::from("dist"),
        };

        let session_ttl_secs = parse_u64_env("RUSTIC_SESSION_TTL_SECS", 60 * 60 * 24 * 7);
        let login_max_attempts = parse_u64_env("RUSTIC_LOGIN_MAX_ATTEMPTS", 5) as u32;
        let login_lockout_secs = parse_u64_env("RUSTIC_LOGIN_LOCKOUT_SECS", 300);

        // Strip a leading dot so both `preview.example.com` and
        // `.preview.example.com` are accepted; matching adds the dot back.
        let preview_domain = std::env::var("RUSTIC_PREVIEW_DOMAIN")
            .ok()
            .map(|s| s.trim().trim_start_matches('.').to_string())
            .filter(|s| !s.is_empty());
        let cookie_domain = std::env::var("RUSTIC_COOKIE_DOMAIN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(Self {
            auth_password,
            session_secret,
            bind_addr,
            data_dir,
            static_dir,
            session_ttl_secs,
            login_max_attempts,
            login_lockout_secs,
            preview_domain,
            cookie_domain,
        })
    }
}

fn parse_u64_env(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Platform default data dir (`~/.local/share/rustic` on Linux, the OS data dir
/// elsewhere). Falls back to `./rustic-data` if no home can be resolved.
fn default_data_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share/rustic");
        }
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("rustic");
        }
    }
    PathBuf::from("rustic-data")
}

/// 32 bytes from the OS CSPRNG for the session-secret fallback.
fn random_secret() -> Vec<u8> {
    let mut out = vec![0u8; 32];
    if rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut out)
        .is_err()
    {
        panic!("OS secure random unavailable; set RUSTIC_SESSION_SECRET");
    }
    out
}
