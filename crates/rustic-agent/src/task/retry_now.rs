//! "Retry now" requests for tasks waiting out a stream-retry backoff.
//!
//! The executor's retry loop sleeps between attempts (up to 90s). The UI's
//! retry banner offers a "Retry now" button; the host command flips a flag
//! here and the executor's backoff sleep polls it to cut the wait short.
//! Keyed by task id so parallel tasks don't wake each other.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

fn map() -> &'static Mutex<HashSet<String>> {
    static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Records a user request to retry `task_id`'s provider call immediately.
pub fn request(task_id: &str) {
    if let Ok(mut m) = map().lock() {
        m.insert(task_id.to_string());
    }
}

/// Consumes a pending retry-now request for `task_id`, returning whether one existed.
pub fn take(task_id: &str) -> bool {
    map().lock().map(|mut m| m.remove(task_id)).unwrap_or(false)
}
