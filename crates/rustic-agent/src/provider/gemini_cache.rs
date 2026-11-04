//! Explicit Gemini context caching (`cachedContents`) for the static request
//! prefix — systemInstruction + tools + toolConfig.
//!
//! Lifecycle:
//! - Keyed by a hash of (base, model, api key, prefix JSON), process-wide.
//! - Only created once the same prefix has been seen on a second request
//!   (i.e. a multi-iteration task) and it is large enough to qualify.
//! - Created with a short TTL; recreated when it is about to expire.
//! - Any creation failure parks the key for a while (fall back silently to
//!   the uncached request, which still benefits from implicit caching).
//! - The caller invalidates the key when a request using the cache is
//!   rejected, then retries uncached.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Conservative minimum (Gemini Pro-class models need 4096 tokens; Flash
/// accepts less). Estimated as chars / 4.
const MIN_CACHE_TOKENS: usize = 4096;
const TTL_SECS: u64 = 600;
/// Treat an entry as expired this long before its real expiry so a request
/// never races the server-side deletion.
const EXPIRY_MARGIN_SECS: u64 = 60;
const FAILURE_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// Fields moved into the cache and stripped from requests that use it.
pub const PREFIX_FIELDS: [&str; 3] = ["systemInstruction", "tools", "toolConfig"];

enum Entry {
    /// Prefix seen once; create on the next sighting.
    Seen,
    Ready { name: String, expires: Instant },
    Failed { until: Instant },
}

static CACHES: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Native Google endpoint only — gateways may not implement cachedContents.
pub fn eligible_base(base: &str) -> bool {
    base.contains("generativelanguage.googleapis.com")
}

/// Static prefix (the fields in [`PREFIX_FIELDS`]) of a request body.
pub fn prefix_of(body: &Value) -> Option<serde_json::Map<String, Value>> {
    let obj = body.as_object()?;
    let mut prefix = serde_json::Map::new();
    for f in PREFIX_FIELDS {
        if let Some(v) = obj.get(f) {
            prefix.insert(f.to_string(), v.clone());
        }
    }
    let size = serde_json::to_string(&prefix).map(|s| s.len()).unwrap_or(0);
    (size / 4 >= MIN_CACHE_TOKENS).then_some(prefix)
}

pub fn cache_key(base: &str, model: &str, api_key: &str, prefix: &serde_json::Map<String, Value>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for part in [base, model, api_key] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.update(serde_json::to_string(prefix).unwrap_or_default().as_bytes());
    hex::encode(h.finalize())
}

enum Action {
    Use(String),
    Create,
    Skip,
}

fn decide(key: &str) -> Action {
    let mut map = CACHES.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match map.get(key) {
        Some(Entry::Ready { name, expires }) if *expires > now => Action::Use(name.clone()),
        Some(Entry::Failed { until }) if *until > now => Action::Skip,
        Some(Entry::Seen) | Some(Entry::Ready { .. }) => Action::Create,
        Some(Entry::Failed { .. }) | None => {
            map.insert(key.to_string(), Entry::Seen);
            Action::Skip
        }
    }
}

fn store(key: &str, entry: Entry) {
    CACHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_string(), entry);
}

/// Mark the cache for `key` unusable (request using it was rejected).
pub fn invalidate(key: &str) {
    store(key, Entry::Failed { until: Instant::now() + FAILURE_BACKOFF });
}

/// Returns a `cachedContents/...` name to use for this request, creating the
/// cache when appropriate. Never errors — `None` means "send uncached".
pub async fn resolve(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    api_key: &str,
    key: &str,
    prefix: serde_json::Map<String, Value>,
) -> Option<String> {
    match decide(key) {
        Action::Use(name) => return Some(name),
        Action::Skip => return None,
        Action::Create => {}
    }
    let mut body = Value::Object(prefix);
    body["model"] = json!(format!("models/{}", model.trim_start_matches("models/")));
    body["ttl"] = json!(format!("{}s", TTL_SECS));
    let url = format!("{}/v1beta/cachedContents", base);
    let res = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("x-goog-api-key", api_key)
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await;
    let name = match res {
        Ok(resp) if resp.status().is_success() => resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string)),
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::info!(
                status = %status,
                error = %text.chars().take(300).collect::<String>(),
                "[gemini-cache] cachedContents create rejected; falling back to uncached"
            );
            None
        }
        Err(e) => {
            tracing::info!(error = %e, "[gemini-cache] cachedContents create failed; falling back");
            None
        }
    };
    match name {
        Some(name) => {
            tracing::info!(model, cache = %name, "[gemini-cache] created explicit context cache");
            store(
                key,
                Entry::Ready {
                    name: name.clone(),
                    expires: Instant::now() + Duration::from_secs(TTL_SECS - EXPIRY_MARGIN_SECS),
                },
            );
            Some(name)
        }
        None => {
            invalidate(key);
            None
        }
    }
}

/// Swap the static prefix in `body` for a `cachedContent` reference and
/// return the removed fields so [`restore_prefix`] can undo it.
pub fn apply_cached(body: &mut Value, name: &str) -> Vec<(String, Value)> {
    let mut removed = Vec::new();
    if let Some(obj) = body.as_object_mut() {
        for f in PREFIX_FIELDS {
            if let Some(v) = obj.remove(f) {
                removed.push((f.to_string(), v));
            }
        }
        obj.insert("cachedContent".to_string(), json!(name));
    }
    removed
}

pub fn restore_prefix(body: &mut Value, removed: Vec<(String, Value)>) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("cachedContent");
        for (k, v) in removed {
            obj.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_prefix_is_not_cached() {
        let body = json!({ "systemInstruction": { "parts": [{ "text": "short" }] }, "contents": [] });
        assert!(prefix_of(&body).is_none());
    }

    #[test]
    fn large_prefix_qualifies_and_round_trips() {
        let big = "x".repeat(MIN_CACHE_TOKENS * 4 + 10);
        let mut body = json!({
            "systemInstruction": { "parts": [{ "text": big }] },
            "tools": [{ "functionDeclarations": [] }],
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
        });
        let prefix = prefix_of(&body).expect("qualifies");
        assert!(prefix.contains_key("tools"));
        let original = body.clone();
        let removed = apply_cached(&mut body, "cachedContents/abc");
        assert_eq!(body["cachedContent"], "cachedContents/abc");
        assert!(body.get("systemInstruction").is_none() && body.get("tools").is_none());
        assert!(body.get("contents").is_some());
        restore_prefix(&mut body, removed);
        assert_eq!(body, original);
    }

    #[test]
    fn create_only_on_second_sighting_and_failures_back_off() {
        let key = "test-key-lifecycle";
        assert!(matches!(decide(key), Action::Skip)); // first sighting
        assert!(matches!(decide(key), Action::Create)); // second → create
        invalidate(key);
        assert!(matches!(decide(key), Action::Skip)); // parked
        store(
            key,
            Entry::Ready { name: "cachedContents/n".into(), expires: Instant::now() + Duration::from_secs(5) },
        );
        assert!(matches!(decide(key), Action::Use(n) if n == "cachedContents/n"));
        store(key, Entry::Ready { name: "cachedContents/old".into(), expires: Instant::now() });
        assert!(matches!(decide(key), Action::Create)); // expired → recreate
    }
}
