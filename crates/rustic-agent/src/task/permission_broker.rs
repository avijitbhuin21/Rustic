use crate::task::{EventTx, PermissionOp, TaskEvent};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Three-state response from the user (or auto-allow path) to a single
/// permission prompt. Mirrors `crate::harness::PermissionDecision` but lives
/// here so the native pipeline doesn't depend on the harness module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePermissionDecision {
    /// Approve this single call; the next matching call will prompt again.
    Accept,
    /// Approve and add the call's signature to the per-task session
    /// allowlist so subsequent matching calls are auto-allowed without
    /// prompting. Plan §5.1 / §B.3.
    AcceptForSession,
    /// Reject the call.
    Deny,
}

/// Mediates per-operation approval in ManualEdit / AutoEdit modes.
///
/// When a tool needs user approval, it calls `request()`, which:
///   1. Checks the per-task session allowlist — if a matching signature was
///      previously approved with `AcceptForSession`, return `true` straight
///      away without bothering the user again.
///   2. Otherwise emits a `TaskEvent::PermissionRequest` to the UI and
///      suspends until the user responds (waits up to 24 hours).
///
/// The Tauri `respond_to_permission` command calls `respond()` (legacy bool)
/// or `respond_with_decision()` (three-state) to unblock the tool.
pub struct PermissionBroker {
    pending: Mutex<HashMap<String, PendingRequest>>,
    /// task_id → set of signatures the user has approved for the rest of
    /// this task's session. Cleared on task delete via `clear_for_task`.
    session_allowlist: Mutex<HashMap<String, HashSet<String>>>,
}

struct PendingRequest {
    sender: oneshot::Sender<bool>,
    task_id: String,
    /// Signature for `AcceptForSession` to remember. `None` for ops that
    /// must never be auto-allowed (sensitive-file tiers).
    signature: Option<String>,
}

/// Derive the session-allowlist signature for `op`.
///
/// The signature controls how broadly "Allow for session" carries forward.
/// Choices made here:
/// * Plain writes/creates collapse to one signature each (`write_file` /
///   `create_file`) — matches "I trust this agent to write files" intent.
/// * `RunCommand` keys by the basename of the first whitespace-delimited
///   token (`/usr/bin/npm install …` → `run_command:npm`). The card shows
///   the full command, so the user sees what they're approving; trusting
///   the *binary* for the session bounds blast radius without nagging
///   per-flag.
/// * `SensitiveFile` returns `None` — these are gated by an explicit
///   security tier and must be re-confirmed each call regardless of the
///   user's earlier choices.
fn signature_for_op(op: &PermissionOp) -> Option<String> {
    match op {
        PermissionOp::WriteFile(_) => Some("write_file".to_string()),
        PermissionOp::CreateFile(_) => Some("create_file".to_string()),
        PermissionOp::RunCommand(cmd) => {
            let trimmed = cmd.trim();
            if trimmed.is_empty() {
                return None;
            }
            let first = trimmed.split_whitespace().next().unwrap_or("");
            // Strip path so `/usr/bin/npm` and `npm` collapse to one entry.
            // `rsplit` over both unix and windows separators handles either.
            let bin = first.rsplit(['/', '\\']).next().unwrap_or(first);
            if bin.is_empty() {
                None
            } else {
                Some(format!("run_command:{}", bin))
            }
        }
        PermissionOp::SensitiveFile { .. } => None,
    }
}

impl PermissionBroker {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            session_allowlist: Mutex::new(HashMap::new()),
        }
    }

    /// Emit a permission request and wait for the user to approve or deny.
    /// Returns `true` if approved (either by the user or by a prior session
    /// rule), `false` if denied or timed out.
    pub async fn request(&self, event_tx: &EventTx, task_id: &str, op: PermissionOp) -> bool {
        let signature = signature_for_op(&op);

        // Auto-allow if this op's signature was previously approved with
        // AcceptForSession for this task. Skip the prompt entirely.
        if let Some(sig) = signature.as_ref() {
            if self.is_allowed_for_session(task_id, sig) {
                tracing::debug!(
                    task = task_id,
                    signature = sig,
                    "permission auto-allowed by session rule"
                );
                return true;
            }
        }

        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(
                request_id.clone(),
                PendingRequest {
                    sender: tx,
                    task_id: task_id.to_string(),
                    signature,
                },
            );
        }

        let (operation, description, preview) = op.describe();
        let _ = event_tx.try_send(TaskEvent::PermissionRequest {
            task_id: task_id.to_string(),
            request_id: request_id.clone(),
            operation,
            description,
            preview,
        });

        match tokio::time::timeout(Duration::from_secs(86400), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                // Timeout or channel closed — auto-deny and clean up
                let mut pending = self.pending.lock().unwrap();
                pending.remove(&request_id);
                false
            }
        }
    }

    /// Legacy two-state response, used by call sites that haven't been
    /// updated for the three-button UX. Treats `approved=true` as
    /// `Accept` (one-shot — does NOT add to the session allowlist).
    pub fn respond(&self, request_id: &str, approved: bool) {
        self.respond_with_decision(
            request_id,
            if approved {
                NativePermissionDecision::Accept
            } else {
                NativePermissionDecision::Deny
            },
        );
    }

    /// Three-state response. `AcceptForSession` records the request's
    /// signature in the per-task allowlist before unblocking the waiting
    /// tool — subsequent matching ops are auto-allowed without prompting.
    pub fn respond_with_decision(
        &self,
        request_id: &str,
        decision: NativePermissionDecision,
    ) {
        let pending = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(request_id)
        };
        let Some(req) = pending else { return };

        if matches!(decision, NativePermissionDecision::AcceptForSession) {
            if let Some(sig) = req.signature.clone() {
                let mut allowlist = self.session_allowlist.lock().unwrap();
                allowlist
                    .entry(req.task_id.clone())
                    .or_default()
                    .insert(sig);
            }
        }

        let approved = !matches!(decision, NativePermissionDecision::Deny);
        let _ = req.sender.send(approved);
    }

    /// Drop every session rule recorded for `task_id`. Called from
    /// `delete_task` so a deleted task's rules don't leak — task ids are
    /// UUIDs so reuse is unlikely, but cleaning up is cheap insurance.
    pub fn clear_for_task(&self, task_id: &str) {
        let mut allowlist = self.session_allowlist.lock().unwrap();
        allowlist.remove(task_id);
    }

    fn is_allowed_for_session(&self, task_id: &str, signature: &str) -> bool {
        let allowlist = self.session_allowlist.lock().unwrap();
        allowlist
            .get(task_id)
            .map(|set| set.contains(signature))
            .unwrap_or(false)
    }
}

impl Default for PermissionBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_collapses_writes_and_creates() {
        assert_eq!(
            signature_for_op(&PermissionOp::WriteFile("/a/b.rs".into())),
            Some("write_file".into())
        );
        assert_eq!(
            signature_for_op(&PermissionOp::CreateFile("/a/b.rs".into())),
            Some("create_file".into())
        );
    }

    #[test]
    fn signature_for_run_command_uses_basename_of_first_word() {
        let cases = [
            ("npm install", "run_command:npm"),
            ("/usr/bin/npm test", "run_command:npm"),
            ("C:\\bin\\git.exe status", "run_command:git.exe"),
            ("   ls -la   ", "run_command:ls"),
        ];
        for (cmd, expected) in cases {
            assert_eq!(
                signature_for_op(&PermissionOp::RunCommand(cmd.into())),
                Some(expected.to_string()),
                "input: {cmd}"
            );
        }
    }

    #[test]
    fn signature_run_command_empty_yields_none() {
        assert_eq!(
            signature_for_op(&PermissionOp::RunCommand("".into())),
            None
        );
        assert_eq!(
            signature_for_op(&PermissionOp::RunCommand("   ".into())),
            None
        );
    }

    #[test]
    fn signature_sensitive_never_session_allowed() {
        assert_eq!(
            signature_for_op(&PermissionOp::SensitiveFile {
                path: "/etc/passwd".into(),
                tier: 1,
                reason: "system file".into(),
            }),
            None
        );
    }
}
