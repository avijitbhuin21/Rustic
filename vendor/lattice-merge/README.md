# lattice-merge

Three-way structural merge and hidden-conflict detection over source text. No
repository, no on-disk format, no `.lat/` directory — give it three strings and
a language name.

Extracted from [Lattice](../../README.md), where it is layer 2 of the land
pipeline, so that other tools can use the parts that are generally useful.

## What it does

```rust
use lattice_merge::{merge_file_checked, MergeStatus};

let checked = merge_file_checked("rust", base, left, right)?;
assert_eq!(checked.outcome.status, MergeStatus::Clean);
// Names defined in `base`, gone from the merge, yet still referenced by it.
assert!(checked.hidden.is_empty());
```

- `merge_file` — line merge fast path, then per-hunk strategies (identical,
  format-only, commutative insert), then name-matched declaration merge. CST
  machinery is only paid for on residual conflicts.
- `merge_file_idaware` — additionally composes a rename on one side with a body
  edit on the other, given the typed rename ops.
- `hidden_conflicts` — the failure mode git merges clean and green: one side
  deletes or renames a symbol while another adds a reference to it.
- `merge_file_checked` — both of the above in one call.

Supported languages: Rust, TypeScript, TSX, JavaScript, Python.

## Merging a whole file set

`merge_file*` works one file at a time, which means you supply the rename ops
yourself. `engine_b::merge_texts` does the set-wise job instead: it derives
stable symbol identities from the base, diffs both sides into typed ops against
that shared frame, and uses them to compose edits that a per-file merge would
have to reject.

```rust
use lattice_merge::engine_b::merge_texts;

// base / ours / theirs are each a BTreeMap<String, String> of path -> text.
let result = merge_texts(&base, &ours, &theirs);
if result.is_clean() {
    for (path, text) in &result.merged { /* write it out */ }
} else {
    // result.conflicts — marked-up text, per path
    // result.hidden    — references the merge left dangling
    // result.checks    — duplicate definitions and stale call arities
}
```

This is what buys you rename-vs-body-edit and cross-file move-vs-edit
composition, neither of which a single-file merge can see. It needs no
repository and no identity table of your own — identities are derived from the
base text on each call.

## Bring your own symbol index

Hidden-conflict detection is name-based (no type info, no arity awareness, no
cross-language references). If you already have a real symbol index, implement
`SymbolExtractor` and pass it to `hidden_conflicts_with` /
`merge_file_checked_with` instead of the bundled tree-sitter extractor.

## Caveat worth reading

A clean merge is **not** a proof of semantic correctness — it composes edits, it
does not understand them. On the phase-0 corpus, 18 clean merges differed from
what the human actually committed. Run your build and tests on the result.

## Requirements

None beyond the Rust toolchain. The line-merge layer runs in-process on
[`diffy`](https://crates.io/crates/diffy) — no `git` on `PATH`, no subprocess and
no temp files.

License: MIT OR Apache-2.0
