pub mod conflict;
pub mod diff;
pub mod git_cli;
pub mod io_util;
pub mod log;
pub mod remote;
pub mod repo;
pub mod status;

pub use conflict::{ConflictFile, ConflictHunk, ConflictSide};
pub use diff::{DiffHunk, DiffLine, FileDiff};
pub use log::{CommitFileChange, CommitInfo};
pub use remote::{clone_repo, clone_repo_with_progress, AheadBehind};
pub use repo::{BranchInfo, GitRepo};
pub use status::{FileStatus, GitStatus, StatusType};

/// Re-exports for hosts that want to check/display the missing-git message
/// without depending on the `git_cli` submodule directly.
pub use git_cli::{is_git_available, GIT_NOT_FOUND_MESSAGE};
