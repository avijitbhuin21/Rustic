pub mod buffer;
pub mod config;
pub mod formatter;
pub mod io_util;
pub mod lsp;
pub mod search;
pub mod syntax;
pub mod workspace;

// Re-export the tree-sitter types that callers (src-tauri) need to construct
// `InputEdit` and `Point` values for incremental highlighter updates. Avoids
// every consumer crate having to add tree-sitter as a direct dep.
pub use tree_sitter;
