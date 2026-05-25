//! Cross-task budget gates.
//!
//! Concurrent provider streams (tokio `Semaphore`, default 6) and daily cost
//! ceiling (USD cents, reset at midnight UTC). `None` on either field in
//! `BudgetSettings` disables that gate. Harness-mode costs are excluded.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default cap if the user hasn't customized the settings.
pub const DEFAULT_MAX_CONCURRENT_STREAMS: usize = 6;

/// User-tunable budget knobs. `None` on either gate field disables it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct BudgetSettings {
    /// Max parallel provider streams across ALL tasks + their sub-agents.
    /// `None` → no gate (acquire is a no-op).
    #[serde(default)]
    pub max_concurrent_streams: Option<usize>,
    /// Daily ceiling in USD cents. `None` → no ceiling (spending is logged
    /// to the running counter but never blocks).
    #[serde(default)]
    pub daily_cost_ceiling_cents: Option<u64>,
    /// Per-task sub-agent concurrency cap. `None` → uncapped. Missing from
    /// older configs deserialises to `Some(DEFAULT_MAX_CONCURRENT_SUBAGENTS)`.
    #[serde(default = "default_max_concurrent_subagents_field")]
    pub max_concurrent_subagents: Option<usize>,
}

/// Default sub-agent concurrency cap.
pub const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 10;

fn default_max_concurrent_subagents_field() -> Option<usize> {
    Some(DEFAULT_MAX_CONCURRENT_SUBAGENTS)
}

/// Process-wide budget enforcer. Cheap to clone — every Arc internally.
#[derive(Clone)]
pub struct Budget {
    semaphore: Option<Arc<Semaphore>>,
    /// Cents spent today against this budget. Reset to 0 when
    /// `current_day_unix` advances past the stored day.
    cents_spent_today: Arc<AtomicU64>,
    /// Unix timestamp of the start of the current UTC day; counter resets when this advances.
    current_day_unix: Arc<AtomicI64>,
    /// Ceiling in cents; `0` = no enforcement. Atomic so it can be raised live
    /// from the breach modal without rebuilding the `Budget`.
    daily_ceiling_cents: Arc<AtomicU64>,
}

/// Result of a `check_within_ceiling` call.
#[derive(Debug, Clone, Copy)]
pub enum CeilingCheck {
    /// Either no ceiling is configured, or today's spend is still under it.
    Allowed,
    /// Today's spend has hit or exceeded the ceiling.
    Blocked {
        ceiling_cents: u64,
        spent_cents: u64,
    },
}

impl Budget {
    pub fn new(settings: &BudgetSettings) -> Self {
        let semaphore = settings
            .max_concurrent_streams
            .filter(|n| *n > 0)
            .map(|n| Arc::new(Semaphore::new(n)));
        Self {
            semaphore,
            cents_spent_today: Arc::new(AtomicU64::new(0)),
            current_day_unix: Arc::new(AtomicI64::new(start_of_utc_today().timestamp())),
            // `0` is the sentinel for "no ceiling" — see field doc.
            daily_ceiling_cents: Arc::new(AtomicU64::new(
                settings.daily_cost_ceiling_cents.unwrap_or(0),
            )),
        }
    }

    /// P0.4 fix #4: raise the in-memory ceiling on a live Budget. Used by
    /// the breach-resolution flow to bump the cap without rebuilding the
    /// Budget — the parked executor's retry of `check_within_ceiling`
    /// observes the new value through the Arc<AtomicU64> handle.
    ///
    /// Note: this does NOT persist the new ceiling to disk. The Tauri
    /// command that calls this is responsible for also updating
    /// `ai_config.budget.daily_cost_ceiling_cents` so subsequent tasks
    /// pick up the new value too.
    pub fn raise_ceiling(&self, new_cents: u64) {
        self.daily_ceiling_cents.store(new_cents, Ordering::SeqCst);
    }

    /// Convenience constructor matching `BudgetSettings::default()` — gates
    /// disabled, counter zeroed. Used by tests and as a fallback when
    /// settings can't be loaded.
    pub fn unrestricted() -> Self {
        Self::new(&BudgetSettings::default())
    }

    /// Acquire a stream permit (blocking on contention). Returns a permit
    /// the caller drops when the provider call finishes. If no semaphore
    /// is configured, returns `None` and runs immediately.
    pub async fn acquire_stream_permit(&self) -> Option<OwnedSemaphorePermit> {
        match &self.semaphore {
            Some(sem) => sem
                .clone()
                .acquire_owned()
                .await
                .ok(),
            None => None,
        }
    }

    /// Roll the daily counter if we've crossed into a new UTC day, then
    /// return whether today's spend is within the configured ceiling.
    /// No mutation to the counter happens here — that's `record_cost`'s job.
    pub fn check_within_ceiling(&self) -> CeilingCheck {
        self.maybe_roll_day();
        let ceiling = self.daily_ceiling_cents.load(Ordering::Relaxed);
        if ceiling == 0 {
            return CeilingCheck::Allowed;
        }
        let spent = self.cents_spent_today.load(Ordering::Relaxed);
        if spent >= ceiling {
            CeilingCheck::Blocked {
                ceiling_cents: ceiling,
                spent_cents: spent,
            }
        } else {
            CeilingCheck::Allowed
        }
    }

    /// Add `cost_usd` to today's tally. Called by the executor after every
    /// provider turn completes. Idempotent at the second-precision day
    /// boundary — if the call straddles midnight the cost lands on whichever
    /// day the call observes after `maybe_roll_day` runs.
    pub fn record_cost(&self, cost_usd: f64) {
        if !cost_usd.is_finite() || cost_usd <= 0.0 {
            return;
        }
        self.maybe_roll_day();
        let cents = (cost_usd * 100.0).round() as u64;
        self.cents_spent_today
            .fetch_add(cents, Ordering::Relaxed);
    }

    /// Inspect today's running totals without recording anything. Used by
    /// the UI to render the cost-so-far number on the budget panel.
    pub fn snapshot(&self) -> (u64, Option<u64>) {
        self.maybe_roll_day();
        let ceiling = self.daily_ceiling_cents.load(Ordering::Relaxed);
        (
            self.cents_spent_today.load(Ordering::Relaxed),
            if ceiling == 0 { None } else { Some(ceiling) },
        )
    }

    /// Compare the start-of-day for "now" against the stored day. If they
    /// differ, atomically reset the cents counter and advance the day
    /// stamp. Race-tolerant: two threads crossing midnight simultaneously
    /// both perform the swap; whichever runs first wins, the other's
    /// store-on-mismatch is a harmless overwrite to the same value.
    fn maybe_roll_day(&self) {
        let today_ts = start_of_utc_today().timestamp();
        let stored = self.current_day_unix.load(Ordering::Relaxed);
        if today_ts != stored {
            // Compare-and-swap the day, then zero the counter. The CAS
            // ensures only one thread does the reset per midnight.
            if self
                .current_day_unix
                .compare_exchange(stored, today_ts, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                self.cents_spent_today.store(0, Ordering::SeqCst);
            }
        }
    }
}

fn start_of_utc_today() -> DateTime<Utc> {
    let now = Utc::now();
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .unwrap_or(now.with_hour(0).unwrap_or(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn unrestricted_passes_everything() {
        let b = Budget::unrestricted();
        // No ceiling → always allowed.
        assert!(matches!(b.check_within_ceiling(), CeilingCheck::Allowed));
        b.record_cost(123.45);
        assert!(matches!(b.check_within_ceiling(), CeilingCheck::Allowed));
        let (spent, ceiling) = b.snapshot();
        assert_eq!(spent, 12345);
        assert!(ceiling.is_none());
    }

    #[test]
    fn ceiling_blocks_when_spend_reaches_cap() {
        let b = Budget::new(&BudgetSettings {
            max_concurrent_streams: None,
            daily_cost_ceiling_cents: Some(100), // $1
            max_concurrent_subagents: None,
        });
        b.record_cost(0.50);
        assert!(matches!(b.check_within_ceiling(), CeilingCheck::Allowed));
        b.record_cost(0.49);
        assert!(matches!(b.check_within_ceiling(), CeilingCheck::Allowed));
        b.record_cost(0.02);
        match b.check_within_ceiling() {
            CeilingCheck::Blocked {
                ceiling_cents,
                spent_cents,
            } => {
                assert_eq!(ceiling_cents, 100);
                assert!(spent_cents >= 100);
            }
            CeilingCheck::Allowed => panic!("expected Blocked, got Allowed"),
        }
    }

    #[test]
    fn record_cost_ignores_non_positive_or_nan() {
        let b = Budget::unrestricted();
        b.record_cost(0.0);
        b.record_cost(-5.0);
        b.record_cost(f64::NAN);
        b.record_cost(f64::INFINITY);
        assert_eq!(b.snapshot().0, 0);
    }

    #[tokio::test]
    async fn semaphore_serialises_excess_requests() {
        let b = Budget::new(&BudgetSettings {
            max_concurrent_streams: Some(1),
            daily_cost_ceiling_cents: None,
            max_concurrent_subagents: None,
        });
        let p1 = b.acquire_stream_permit().await.expect("permit");
        // Second acquire must wait; verify it does NOT complete before we drop p1.
        let b2 = b.clone();
        let handle = tokio::spawn(async move {
            let _p2 = b2.acquire_stream_permit().await;
            std::time::Instant::now()
        });
        let started = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!handle.is_finished(), "second acquire should still be blocked");
        drop(p1);
        let acquired_at = handle.await.expect("task");
        let waited = acquired_at.duration_since(started);
        assert!(waited >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn no_semaphore_returns_none_permit() {
        let b = Budget::new(&BudgetSettings {
            max_concurrent_streams: None,
            daily_cost_ceiling_cents: None,
            max_concurrent_subagents: None,
        });
        assert!(b.acquire_stream_permit().await.is_none());
    }

    #[test]
    fn day_rollover_resets_counter() {
        let b = Budget::unrestricted();
        b.record_cost(5.00);
        assert_eq!(b.snapshot().0, 500);
        // Simulate yesterday's date being stored — next maybe_roll_day call
        // should observe today != stored and zero the counter.
        b.current_day_unix.store(0, Ordering::SeqCst);
        b.maybe_roll_day();
        assert_eq!(b.snapshot().0, 0);
    }
}
