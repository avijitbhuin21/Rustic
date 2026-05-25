pub mod repo;
pub mod status;
pub mod diff;
pub mod remote;
pub mod conflict;
pub mod io_util;
pub mod log;
pub mod git_cli;

pub use repo::{BranchInfo, GitRepo};
pub use status::{FileStatus, GitStatus, StatusType};
pub use diff::{DiffHunk, DiffLine, FileDiff};
pub use remote::{AheadBehind, clone_repo};
pub use conflict::{ConflictFile, ConflictHunk, ConflictSide};
pub use log::{CommitInfo, CommitFileChange};

/// Re-exports for hosts that want to check/display the missing-git message
/// without depending on the `git_cli` submodule directly.
pub use git_cli::{is_git_available, GIT_NOT_FOUND_MESSAGE};
