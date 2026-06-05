//! Authentication: HMAC-signed session tokens, constant-time password check,
//! and a per-IP login rate limiter.
//!
//! Token format: `<exp>.<hex-hmac>` where `hmac = HMAC-SHA256(session_secret,
//! exp_ascii)` and `exp` is a unix-seconds expiry. Stateless — no server-side
//! session table — which suits the single-user model: any token the server
//! itself signed and that hasn't expired is valid. Rotating
//! `RUSTIC_SESSION_SECRET` invalidates all outstanding tokens.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sign(secret: &[u8], msg: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Issue a session token valid for `ttl_secs` from now.
pub fn issue_token(secret: &[u8], ttl_secs: u64) -> String {
    let exp = now_secs() + ttl_secs;
    let exp_str = exp.to_string();
    let sig = sign(secret, &exp_str);
    format!("{exp_str}.{sig}")
}

/// Verify a token: well-formed, signature matches (constant-time), not expired.
pub fn verify_token(secret: &[u8], token: &str) -> bool {
    let Some((exp_str, sig)) = token.split_once('.') else {
        return false;
    };
    let Ok(exp) = exp_str.parse::<u64>() else {
        return false;
    };
    let expected = sign(secret, exp_str);
    // Constant-time compare to avoid leaking the signature byte-by-byte.
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() != 1 {
        return false;
    }
    exp > now_secs()
}

/// Constant-time password comparison against the configured password.
pub fn password_matches(configured: &str, attempt: &str) -> bool {
    configured.as_bytes().ct_eq(attempt.as_bytes()).unwrap_u8() == 1
}

/// Simple in-memory per-IP login throttle. After `max_attempts` consecutive
/// failures an IP is locked out for `lockout` duration; a success resets it.
pub struct RateLimiter {
    max_attempts: u32,
    lockout: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    failures: u32,
    locked_until: Option<Instant>,
}

impl RateLimiter {
    pub fn new(max_attempts: u32, lockout_secs: u64) -> Self {
        Self {
            max_attempts,
            lockout: Duration::from_secs(lockout_secs),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `Some(remaining_secs)` if the IP is currently locked out.
    pub fn locked_for(&self, ip: &str) -> Option<u64> {
        let mut map = self.entries.lock().ok()?;
        let entry = map.get_mut(ip)?;
        match entry.locked_until {
            Some(until) if until > Instant::now() => {
                Some((until - Instant::now()).as_secs() + 1)
            }
            Some(_) => {
                // Lockout expired — reset so the next attempt starts fresh.
                entry.failures = 0;
                entry.locked_until = None;
                None
            }
            None => None,
        }
    }

    pub fn record_failure(&self, ip: &str) {
        let Ok(mut map) = self.entries.lock() else {
            return;
        };
        let entry = map.entry(ip.to_string()).or_insert(Entry {
            failures: 0,
            locked_until: None,
        });
        entry.failures += 1;
        if entry.failures >= self.max_attempts {
            entry.locked_until = Some(Instant::now() + self.lockout);
        }
    }

    pub fn record_success(&self, ip: &str) {
        if let Ok(mut map) = self.entries.lock() {
            map.remove(ip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip() {
        let secret = b"test-secret";
        let token = issue_token(secret, 60);
        assert!(verify_token(secret, &token));
        // Wrong secret fails.
        assert!(!verify_token(b"other-secret", &token));
        // Tampered signature fails.
        let bad = format!("{}.deadbeef", token.split_once('.').unwrap().0);
        assert!(!verify_token(secret, &bad));
        // Garbage fails.
        assert!(!verify_token(secret, "not-a-token"));
    }

    #[test]
    fn expired_token_rejected() {
        let secret = b"s";
        // exp in the past: hand-craft with ttl 0, then it's exp == now, which is
        // not strictly greater than now → rejected on the same or next second.
        let exp = now_secs() - 10;
        let token = format!("{exp}.{}", sign(secret, &exp.to_string()));
        assert!(!verify_token(secret, &token));
    }

    #[test]
    fn password_check_is_exact() {
        assert!(password_matches("hunter2", "hunter2"));
        assert!(!password_matches("hunter2", "hunter3"));
        assert!(!password_matches("hunter2", "hunter2 "));
    }

    #[test]
    fn rate_limiter_locks_out() {
        let rl = RateLimiter::new(3, 300);
        assert_eq!(rl.locked_for("1.2.3.4"), None);
        rl.record_failure("1.2.3.4");
        rl.record_failure("1.2.3.4");
        assert_eq!(rl.locked_for("1.2.3.4"), None);
        rl.record_failure("1.2.3.4");
        assert!(rl.locked_for("1.2.3.4").is_some());
        rl.record_success("1.2.3.4");
        assert_eq!(rl.locked_for("1.2.3.4"), None);
    }
}
