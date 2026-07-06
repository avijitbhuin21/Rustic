//! Authentication: HMAC-signed session tokens, constant-time password check,
//! and a per-IP login rate limiter.
//!
//! Token format: `<exp>.<gen>.<hex-hmac>` where `hmac = HMAC-SHA256(
//! session_secret, "<exp>.<gen>")`, `exp` is a unix-seconds expiry, and `gen`
//! is the server's session generation at issue time. Stateless — no server-side
//! session table — which suits the single-user model: any token the server
//! itself signed, that hasn't expired, AND whose generation still matches the
//! live counter is valid.
//!
//! Two things invalidate outstanding tokens: rotating `RUSTIC_SESSION_SECRET`
//! (changes the signing key) and bumping the session generation (what the
//! "power off" / logout flow does — see `commands::power`). The generation is
//! persisted in the DB so a logout survives a server restart.

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

/// Issue a session token valid for `ttl_secs` from now, bound to session
/// generation `gen`. Bumping the live generation later invalidates this token.
pub fn issue_token(secret: &[u8], ttl_secs: u64, gen: u64) -> String {
    let exp = now_secs() + ttl_secs;
    let payload = format!("{exp}.{gen}");
    let sig = sign(secret, &payload);
    format!("{payload}.{sig}")
}

/// Verify a token: well-formed, signature matches (constant-time), not expired,
/// and its embedded generation still matches `current_gen`.
pub fn verify_token(secret: &[u8], current_gen: u64, token: &str) -> bool {
    // `exp.gen.sig` — exp/gen are digits and sig is hex, so neither contains a
    // dot; a clean three-way split is unambiguous.
    let mut parts = token.split('.');
    let (Some(exp_str), Some(gen_str), Some(sig), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Ok(exp) = exp_str.parse::<u64>() else {
        return false;
    };
    let Ok(gen) = gen_str.parse::<u64>() else {
        return false;
    };
    let payload = format!("{exp_str}.{gen_str}");
    let expected = sign(secret, &payload);
    // Constant-time compare to avoid leaking the signature byte-by-byte.
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() != 1 {
        return false;
    }
    gen == current_gen && exp > now_secs()
}

/// Constant-time password comparison against the configured password.
pub fn password_matches(configured: &str, attempt: &str) -> bool {
    configured.as_bytes().ct_eq(attempt.as_bytes()).unwrap_u8() == 1
}

/// One-time WebSocket-auth tickets.
///
/// Browser `WebSocket` can't set an `Authorization` header, so the upgrade
/// request must carry its credential in the URL. Carrying the long-lived
/// session token there leaks it to anything that logs query strings (reverse
/// proxies, access logs). Instead the client POSTs `/api/ws_ticket` with its
/// normal credentials and receives a cryptographically random, single-use,
/// short-TTL ticket bound to the current session generation; only that ticket
/// rides in the URL, and it is consumed on first use.
pub struct TicketStore {
    tickets: Mutex<HashMap<String, Ticket>>,
}

struct Ticket {
    expires: Instant,
    gen: u64,
}

/// Tickets are redeemed within milliseconds of issue (the client connects the
/// WebSocket immediately); 30s absorbs slow links and clock-free retries.
const TICKET_TTL: Duration = Duration::from_secs(30);

/// Hard cap so the map can't grow without bound even under a request flood
/// from an authenticated-but-misbehaving client. Past the cap the
/// soonest-expiring ticket is evicted.
const MAX_TICKETS: usize = 4096;

impl Default for TicketStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TicketStore {
    pub fn new() -> Self {
        Self {
            tickets: Mutex::new(HashMap::new()),
        }
    }

    /// Issue a fresh single-use ticket bound to session generation `gen`.
    pub fn issue(&self, gen: u64) -> String {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let ticket = hex::encode(bytes);

        let Ok(mut map) = self.tickets.lock() else {
            return ticket; // poisoned lock: ticket simply won't redeem
        };
        let now = Instant::now();
        map.retain(|_, t| t.expires > now); // opportunistic purge
        if map.len() >= MAX_TICKETS {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, t)| t.expires)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(
            ticket.clone(),
            Ticket {
                expires: now + TICKET_TTL,
                gen,
            },
        );
        ticket
    }

    /// Redeem a ticket: it must exist, be unexpired, and match the live
    /// session generation. The ticket is consumed (removed) regardless of the
    /// generation check, so it can never be replayed.
    pub fn redeem(&self, ticket: &str, current_gen: u64) -> bool {
        let Ok(mut map) = self.tickets.lock() else {
            return false;
        };
        let now = Instant::now();
        map.retain(|_, t| t.expires > now); // opportunistic purge
        match map.remove(ticket) {
            Some(t) => t.expires > now && t.gen == current_gen,
            None => false,
        }
    }
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
    /// When this IP last failed — lets stale entries be evicted.
    last_failure: Instant,
}

/// Cap on tracked IPs: the map is keyed by untrusted input (client IPs, or a
/// spoofable header behind a misconfigured proxy), so it must not grow without
/// bound. Past the cap, stale entries are purged and, at worst, the oldest one
/// is evicted to admit the new key.
const MAX_TRACKED_IPS: usize = 10_000;

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
            Some(until) if until > Instant::now() => Some((until - Instant::now()).as_secs() + 1),
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
        let now = Instant::now();

        // Opportunistic eviction before admitting a new key: drop entries that
        // are neither locked out nor recently active (older than the lockout
        // window); if the map is somehow still full of live entries, evict the
        // single oldest so memory stays bounded no matter what.
        if map.len() >= MAX_TRACKED_IPS && !map.contains_key(ip) {
            let lockout = self.lockout;
            map.retain(|_, e| {
                e.locked_until.map_or(false, |until| until > now)
                    || now.duration_since(e.last_failure) < lockout
            });
            if map.len() >= MAX_TRACKED_IPS {
                if let Some(oldest) = map
                    .iter()
                    .min_by_key(|(_, e)| e.last_failure)
                    .map(|(k, _)| k.clone())
                {
                    map.remove(&oldest);
                }
            }
        }

        let entry = map.entry(ip.to_string()).or_insert(Entry {
            failures: 0,
            locked_until: None,
            last_failure: now,
        });
        entry.failures += 1;
        entry.last_failure = now;
        if entry.failures >= self.max_attempts {
            entry.locked_until = Some(now + self.lockout);
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
        let token = issue_token(secret, 60, 0);
        assert!(verify_token(secret, 0, &token));
        // Wrong secret fails.
        assert!(!verify_token(b"other-secret", 0, &token));
        // Tampered signature fails.
        let prefix = token.rsplit_once('.').unwrap().0;
        let bad = format!("{prefix}.deadbeef");
        assert!(!verify_token(secret, 0, &bad));
        // Garbage fails.
        assert!(!verify_token(secret, 0, "not-a-token"));
    }

    #[test]
    fn generation_bump_invalidates_token() {
        let secret = b"test-secret";
        let token = issue_token(secret, 60, 7);
        // Valid only against the matching generation.
        assert!(verify_token(secret, 7, &token));
        // A bumped generation rejects every previously-issued token.
        assert!(!verify_token(secret, 8, &token));
        assert!(!verify_token(secret, 0, &token));
    }

    #[test]
    fn expired_token_rejected() {
        let secret = b"s";
        // exp in the past: hand-craft with ttl 0, then it's exp == now, which is
        // not strictly greater than now → rejected on the same or next second.
        let exp = now_secs() - 10;
        let payload = format!("{exp}.0");
        let token = format!("{payload}.{}", sign(secret, &payload));
        assert!(!verify_token(secret, 0, &token));
    }

    #[test]
    fn password_check_is_exact() {
        assert!(password_matches("hunter2", "hunter2"));
        assert!(!password_matches("hunter2", "hunter3"));
        assert!(!password_matches("hunter2", "hunter2 "));
    }

    #[test]
    fn ticket_is_single_use_and_gen_bound() {
        let store = TicketStore::new();
        let t = store.issue(3);
        // Wrong generation consumes the ticket and fails.
        assert!(!store.redeem(&t, 4));
        assert!(!store.redeem(&t, 3));
        // Fresh ticket, right generation: succeeds exactly once.
        let t2 = store.issue(3);
        assert!(store.redeem(&t2, 3));
        assert!(!store.redeem(&t2, 3));
        // Unknown ticket fails.
        assert!(!store.redeem("deadbeef", 3));
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
