//! Poison-resilient mutex locking.
//!
//! `std::sync::Mutex` *poisons* itself when a thread panics while holding the
//! guard. After that, every subsequent `lock().unwrap()` on that mutex panics
//! too — so a single panic inside one command permanently bricks that whole
//! subsystem (terminal, workspace, git, buffers, …) until the app is
//! restarted. For a long-lived desktop app that turns a recoverable one-off
//! error into a "restart Rustic" bug, and it's exactly the cascade that the
//! ~100 `state.*.lock().unwrap()` call sites on the IPC surface were exposed
//! to (see app-audit/03-cleanliness-good-practices-audit.md, CLN-02).
//!
//! `lock_safe()` recovers the guard from a poisoned mutex instead of
//! panicking — the same no-poison behaviour `parking_lot::Mutex` provides —
//! so the subsystem keeps working. The data behind the lock still upholds
//! Rust's type invariants (the standard collections stay structurally valid
//! even if an operation panics mid-way); it may be *logically* stale, which is
//! strictly better than a dead subsystem. The original panic that poisoned the
//! lock is unaffected and still surfaces / is logged on its own — recovering
//! here only stops the *secondary* cascade, and emits one error log at the
//! recovery site so a swallowed poison is never fully silent.

use std::sync::{Mutex, MutexGuard};

pub trait MutexExt<T> {
    /// Lock the mutex, recovering the guard if the mutex was poisoned by a
    /// panic in another thread. Prefer this over `lock().unwrap()` everywhere
    /// a poisoned lock should degrade gracefully rather than cascade-panic.
    fn lock_safe(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    #[track_caller]
    fn lock_safe(&self) -> MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!(
                    location = %std::panic::Location::caller(),
                    "recovered from a poisoned mutex (a prior panic left it poisoned); \
                     continuing with possibly-stale state instead of cascading the panic"
                );
                poisoned.into_inner()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MutexExt;
    use std::sync::{Arc, Mutex};

    #[test]
    fn lock_safe_behaves_like_lock_when_healthy() {
        let m = Mutex::new(41);
        *m.lock_safe() += 1;
        assert_eq!(*m.lock_safe(), 42);
    }

    #[test]
    fn lock_safe_recovers_after_poison() {
        let m = Arc::new(Mutex::new(vec![1, 2, 3]));
        let m2 = Arc::clone(&m);
        // Poison the mutex: panic while holding the guard on another thread.
        let _ = std::thread::spawn(move || {
            let _guard = m2.lock().unwrap();
            panic!("boom while holding the lock");
        })
        .join();

        // A plain lock() would now return Err; lock_safe() must still hand
        // back a usable guard so the subsystem keeps working.
        assert!(m.lock().is_err(), "precondition: mutex is poisoned");
        let mut g = m.lock_safe();
        g.push(4);
        assert_eq!(*g, vec![1, 2, 3, 4]);
    }
}
