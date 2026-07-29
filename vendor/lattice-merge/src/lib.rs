//! Pure three-way merge for source text: line merge, hunk strategies,
//! declaration-level merge, and reference-based hidden-conflict detection.
//!
//! Nothing here touches a repository, a store, or an on-disk format — every
//! entry point is a function over `&str`, so a host can depend on this crate
//! alone without a `.lat/` directory existing.

pub mod algebra;
pub mod checks;
pub mod diff_ops;
pub mod engine_b;
pub mod hash;
pub mod hidden;
pub mod linemerge;
pub mod metrics;
pub mod parsers;
pub mod structural_merge;
pub mod symbols;

pub use checks::{
    arity_mismatches, arity_mismatches_in, duplicate_definitions, duplicates_in,
    new_arity_mismatches, new_duplicate_definitions, Arity, CallSite, Definition, Signature,
};
pub use engine_b::{merge_texts, SetMerge};
pub use hidden::{
    hidden_conflicts, merge_file_checked, merge_file_checked_multibase, merge_files_checked,
    CheckCoverage, CheckedMerge, SymbolExtractor, TreeSitterExtractor,
};
pub use parsers::lang_for_path;
pub use structural_merge::{merge_file, merge_file_idaware, MergeOutcome, MergeStatus};
