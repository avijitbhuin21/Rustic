//! Content hashing: blake3, hex-encoded, used as the object id everywhere.
//!
//! A `Hash` is also a *path fragment* — `Store::path_for` splits it into a
//! fanout directory and a filename. That makes it a trust boundary: ids are
//! read verbatim out of `.lat/HEAD` and `.lat/log`, and `sync` copies those
//! lines from a remote. An unvalidated id therefore resolves wherever the
//! attacker points it, and `Store::redact` overwrites whatever it finds
//! (SEC-2). Validation lives on construction and on deserialization so an
//! invalid `Hash` cannot survive a trust boundary.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Length of a hex-encoded blake3 digest.
pub const HEX_LEN: usize = 64;

/// A blake3 content hash rendered as lowercase hex.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Hash(pub String);

/// True for exactly 64 lowercase hex characters and nothing else.
pub fn is_valid_hex(text: &str) -> bool {
    text.len() == HEX_LEN
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Hash {
    /// Hash raw bytes into a Hash.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Hash(blake3::hash(bytes).to_hex().to_string())
    }

    /// Parse an id that came from disk, a remote, or a user. This is the only
    /// constructor that should be used on untrusted text.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let trimmed = text.trim();
        if !is_valid_hex(trimmed) {
            anyhow::bail!(
                "`{trimmed}` is not a valid object id (expected {HEX_LEN} lowercase hex characters)"
            );
        }
        Ok(Hash(trimmed.to_string()))
    }

    /// True when this id is well-formed.
    pub fn is_valid(&self) -> bool {
        is_valid_hex(&self.0)
    }

    /// Return the two-char fanout prefix and the remainder of the hex digest.
    /// Fails rather than panicking on a malformed id: `split_at(2)` aborts the
    /// process on a short id or a non-UTF-8 char boundary.
    pub fn fanout(&self) -> anyhow::Result<(&str, &str)> {
        if !self.is_valid() {
            anyhow::bail!("refusing to resolve a malformed object id `{}`", self.0);
        }
        Ok(self.0.split_at(2))
    }

    /// Return a short 12-char prefix for human display.
    pub fn short(&self) -> &str {
        let end = self
            .0
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(self.0.len()))
            .take_while(|i| *i <= 12)
            .last()
            .unwrap_or(0);
        &self.0[..end]
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if !is_valid_hex(&text) {
            return Err(serde::de::Error::custom(format!(
                "`{text}` is not a valid object id (expected {HEX_LEN} lowercase hex characters)"
            )));
        }
        Ok(Hash(text))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_digests_round_trip() {
        let h = Hash::of_bytes(b"hello");
        assert!(h.is_valid());
        let (fan, rest) = h.fanout().unwrap();
        assert_eq!(fan.len(), 2);
        assert_eq!(rest.len(), 62);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<Hash>(&json).unwrap(), h);
    }

    #[test]
    fn malformed_ids_are_refused_everywhere() {
        for bad in [
            "",
            "a",
            "é",
            "../../etc/passwd",
            "ZZ",
            &"a".repeat(63),
            &"A".repeat(64),
        ] {
            assert!(Hash::parse(bad).is_err(), "{bad:?} must not parse");
            let json = serde_json::to_string(bad).unwrap();
            assert!(
                serde_json::from_str::<Hash>(&json).is_err(),
                "{bad:?} must not deserialize"
            );
            assert!(
                Hash(bad.to_string()).fanout().is_err(),
                "{bad:?} must not resolve"
            );
        }
    }

    #[test]
    fn short_never_panics_on_a_char_boundary() {
        assert_eq!(Hash(String::new()).short(), "");
        assert_eq!(Hash("é".to_string()).short(), "é");
        // Truncation is by bytes, so 12 bytes is four 3-byte characters —
        // the point is that it lands on a boundary rather than panicking.
        assert_eq!(Hash("日本語です".to_string()).short(), "日本語で");
        let long = "a".repeat(64);
        for s in ["日本語です", "ééééééééé", "aé日", long.as_str()] {
            let hash = Hash(s.to_string());
            let short = hash.short();
            assert!(short.len() <= 12, "{s:?} -> {short:?} must fit in 12 bytes");
            assert!(s.starts_with(short), "{s:?} -> {short:?} must be a prefix");
        }
    }
}
