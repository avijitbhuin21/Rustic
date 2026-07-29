//! Merge algebra ported from the validated Phase 0 prototype
//! (`phase0/SPEC.md`): typed ops, per-op prefix contexts, commutation
//! matrix, and the merge function.

pub mod commute;
pub mod merge;
pub mod model;
pub mod ops;

pub use commute::commute;
pub use merge::{merge, MergeResult};
pub use model::{Decl, State, ROOT};
pub use ops::{applicable, apply, apply_all, Op};
