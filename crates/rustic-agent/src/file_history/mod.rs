//! Changed-files tracker (R.1 / shadow-git design).
//!
//! Architecture (see `docs/file_tracking_decision.md`):
//! - Edit/Write/NotebookEdit tools call `tracker::open_snapshot` once per
//!   user message; that snapshot captures the full pre-message worktree
//!   into a libgit2 shadow tree.
//! - Bash tool pushes a `SweepJob` onto an mpsc channel after each
//!   invocation; the sweep worker re-runs `shadow.track()` on a blocking
//!   thread and updates the snapshot's tree_oid in metadata.
//! - Storage: bare libgit2 repo at
//!   `{configDir}/file-history/shadow/<project_hash>/` for tree+blob
//!   objects, plus a thin SQLite metadata layer
//!   (`file_history_snapshots(message_id, task_id, sequence, tree_oid)`).

pub mod accumulator;
pub mod baseline_gate;
pub mod shadow;
pub mod stat_cache;
pub mod sweep;
pub mod tracker;
pub mod walk;
pub mod watcher;

pub use accumulator::{DirtyPathAccumulator, DirtySet};
pub use baseline_gate::{BaselineGate, BaselineState};
pub use watcher::{FileWatcher, FileWatcherError};
pub use shadow::{
    Oid, ShadowError, ShadowRestoreAction, ShadowSnapshot, TrackResult, MAX_TRACKED_FILE_SIZE,
    SYNC_CAPTURE_SOFT_LIMIT,
};
pub use sweep::{ChangeCallback, SweepEnqueueError, SweepJob, SweepWorker};
pub use tracker::{
    CaptureOutcome, FileChangeStats, FileDiff, FileHistory, FileHistoryError, RestoreOutcome,
    RevertPlanEntry, TaskNetChange,
};
// record_final_state and list_task_net_changes_final are methods on FileHistory,
// not free functions, so no extra re-exports are needed here.
pub use walk::{changed_since, join_rel, normalize_rel, walk_for_sweep, WalkedFile};
