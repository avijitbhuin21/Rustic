//! Changed-files tracker.
//!
//! Architecture (see `memory/project_changed_files_tracker.md` for the full
//! design memo):
//! - Edit/Write/NotebookEdit tools call `tracker::capture` synchronously
//!   before mutating a file (~5ms, on the agent path).
//! - Bash tool pushes a `SweepJob` onto an mpsc channel after each invocation;
//!   a single-consumer worker drains it, walks the worktree, and updates the
//!   snapshot. Worker runs on a tokio blocking thread; agent never waits.
//! - Storage: SQLite index (snapshots / files / blobs) + content-addressed
//!   blobs on disk under `{configDir}/file-history/blobs/{hash[:2]}/{hash}`.

pub mod blob_store;
pub mod sweep;
pub mod tracker;
pub mod walk;

pub use blob_store::{BlobStore, BlobStoreError, StoredBlob};
pub use sweep::{ChangeCallback, SweepEnqueueError, SweepJob, SweepWorker};
pub use tracker::{
    CaptureOutcome, FileChangeStats, FileDiff, FileHistory, FileHistoryError, RestoreOutcome,
    RevertPlanEntry, SweepFileChange, TaskNetChange,
};
pub use walk::{changed_since, join_rel, normalize_rel, walk_for_sweep, WalkedFile};
