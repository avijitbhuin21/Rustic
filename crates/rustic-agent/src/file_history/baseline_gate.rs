//! Readiness gate that decouples the file-history baseline capture from the
//! start of an agent turn.
//!
//! Historically `open_snapshot` (a full-worktree hash) was awaited *before* the
//! executor loop started, so the agent's first response was blocked on it. On a
//! large repo that's seconds-to-minutes of dead air. Instead the host now spawns
//! the baseline build in the background and hands the executor a `BaselineGate`.
//! The model can stream its first response immediately; only the **first
//! file-mutating tool** awaits the gate, by which point the baseline has usually
//! finished (it overlaps the model's first round-trip).
//!
//! Correctness: revert semantics are unchanged. The gate guarantees the baseline
//! snapshot row exists before any mutation runs, so `capture` /
//! `record_post_bash_state` still see the pre-message tree. The gate always
//! resolves to a terminal state (`Ready` or `Failed`) — even if the background
//! task panics, `wait()` returns `Failed` rather than hanging the turn.

use tokio::sync::watch;

/// Terminal/transient state of the background baseline capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineState {
    /// Baseline build is still running.
    Pending,
    /// Baseline captured successfully; the snapshot row exists.
    Ready,
    /// Baseline failed (or its task died). Tracking is degraded for this turn,
    /// the same as the legacy "tracker disabled" path; mutations proceed.
    Failed,
}

/// Cheaply-cloneable handle the executor awaits before the first mutating tool.
#[derive(Clone)]
pub struct BaselineGate {
    rx: watch::Receiver<BaselineState>,
}

impl BaselineGate {
    /// Create a pending gate plus the sender the background baseline task uses
    /// to publish the terminal state exactly once.
    pub fn new() -> (watch::Sender<BaselineState>, BaselineGate) {
        let (tx, rx) = watch::channel(BaselineState::Pending);
        (tx, BaselineGate { rx })
    }

    /// A gate that is already resolved `Ready`. Used where there is no baseline
    /// to wait for (plan mode, or a turn with no tracker attached).
    pub fn ready_now() -> BaselineGate {
        // The sender is dropped immediately; `wait()` still returns `Ready`
        // because `wait_for` evaluates the current value before awaiting any
        // change, and the current value already satisfies the predicate.
        let (_tx, rx) = watch::channel(BaselineState::Ready);
        BaselineGate { rx }
    }

    /// Resolve once the baseline reaches a terminal state. Returns instantly if
    /// already resolved, so the executor can call this before every write tool
    /// without bookkeeping. If the sender was dropped before publishing a
    /// terminal state, returns `Failed` (proceed degraded, never hang).
    pub async fn wait(&self) -> BaselineState {
        let mut rx = self.rx.clone();
        // Bind to a local so the borrowing `Ref` guard drops at the `;`, before
        // `rx` does (the tail-expression form keeps the guard alive too long).
        let state = match rx.wait_for(|s| *s != BaselineState::Pending).await {
            Ok(state) => *state,
            Err(_) => BaselineState::Failed,
        };
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ready_now_resolves_immediately() {
        assert_eq!(BaselineGate::ready_now().wait().await, BaselineState::Ready);
    }

    #[tokio::test]
    async fn wait_resolves_after_send() {
        let (tx, gate) = BaselineGate::new();
        let handle = tokio::spawn(async move { gate.wait().await });
        tx.send(BaselineState::Ready).unwrap();
        assert_eq!(handle.await.unwrap(), BaselineState::Ready);
    }

    #[tokio::test]
    async fn dropped_sender_resolves_failed() {
        let (tx, gate) = BaselineGate::new();
        drop(tx); // task died without publishing
        assert_eq!(gate.wait().await, BaselineState::Failed);
    }

    #[tokio::test]
    async fn already_resolved_wait_is_instant_and_repeatable() {
        let (tx, gate) = BaselineGate::new();
        tx.send(BaselineState::Failed).unwrap();
        // Multiple waiters all see the terminal state.
        assert_eq!(gate.wait().await, BaselineState::Failed);
        assert_eq!(gate.clone().wait().await, BaselineState::Failed);
    }
}
