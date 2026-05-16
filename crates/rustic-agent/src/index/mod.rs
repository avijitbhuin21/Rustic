//! P1.2 — Workspace symbol index.
//!
//! Per-project map of `name → [SymbolEntry]` built by running tree-sitter
//! "tags" queries against every source file in the project. Owned by
//! `WorkspaceServices` (P1.3) so multiple concurrent tasks share one index.
//!
//! The build is **lazy + async**: the index is empty when `WorkspaceServices`
//! is constructed; the first agent tool call that needs it triggers a
//! background build via `ensure_built()`. Lookups during the build phase
//! return whatever's indexed so far plus a status flag so callers can warn
//! the user / agent that results are partial.

pub mod builder;
pub mod queries;
pub mod store;
pub mod symbol;

pub use builder::{build_full, refresh_file, IndexBuildStats};
pub use store::{IndexStatus, SymbolIndex};
pub use symbol::{SymbolEntry, SymbolKind};
