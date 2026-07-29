//! The set-wise Engine B entry point used exactly as an external host would:
//! file maps in, merged text out, no repository and no `.lat/` directory.

use lattice_merge::engine_b::merge_texts;
use std::collections::BTreeMap;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(p, t)| ((*p).to_string(), (*t).to_string())).collect()
}

/// Edits to different files commute and both land.
#[test]
fn disjoint_files_from_both_sides_compose() {
    let base = map(&[
        ("a.rs", "fn a() -> i32 {\n    1\n}\n"),
        ("b.rs", "fn b() -> i32 {\n    2\n}\n"),
    ]);
    let ours = map(&[
        ("a.rs", "fn a() -> i32 {\n    10\n}\n"),
        ("b.rs", "fn b() -> i32 {\n    2\n}\n"),
    ]);
    let theirs = map(&[
        ("a.rs", "fn a() -> i32 {\n    1\n}\n"),
        ("b.rs", "fn b() -> i32 {\n    20\n}\n"),
    ]);

    let result = merge_texts(&base, &ours, &theirs);

    assert!(result.conflicts.is_empty(), "unexpected conflicts: {:?}", result.conflicts);
    assert!(result.is_clean(), "expected a clean merge, got {result:?}");
    assert!(result.merged["a.rs"].contains("10"), "ours' edit to a.rs lost");
    assert!(result.merged["b.rs"].contains("20"), "theirs' edit to b.rs lost");
}

/// The Engine B differentiator: one side renames a declaration, the other
/// edits its body. Name-keyed merging sees delete+add versus modify and
/// conflicts; identity-keyed merging composes them.
#[test]
fn rename_on_one_side_composes_with_a_body_edit_on_the_other() {
    let base = map(&[(
        "lib.rs",
        "fn helper(x: i32) -> i32 {\n    x + 1\n}\n\nfn main() {\n    println!(\"{}\", helper(1));\n}\n",
    )]);
    let ours = map(&[(
        "lib.rs",
        "fn assist(x: i32) -> i32 {\n    x + 1\n}\n\nfn main() {\n    println!(\"{}\", assist(1));\n}\n",
    )]);
    let theirs = map(&[(
        "lib.rs",
        "fn helper(x: i32) -> i32 {\n    x + 2\n}\n\nfn main() {\n    println!(\"{}\", helper(1));\n}\n",
    )]);

    let result = merge_texts(&base, &ours, &theirs);

    assert!(result.conflicts.is_empty(), "expected a compose, got {:?}", result.conflicts);
    let merged = &result.merged["lib.rs"];
    assert!(merged.contains("fn assist"), "rename lost:\n{merged}");
    assert!(merged.contains("x + 2"), "body edit lost:\n{merged}");
    assert!(!merged.contains("fn helper"), "old name survived:\n{merged}");
}

/// Two sides rewriting the same line is a genuine conflict and must be
/// reported rather than silently resolved.
#[test]
fn divergent_edits_to_one_line_conflict() {
    let base = map(&[("lib.rs", "fn f() -> i32 {\n    0\n}\n")]);
    let ours = map(&[("lib.rs", "fn f() -> i32 {\n    1\n}\n")]);
    let theirs = map(&[("lib.rs", "fn f() -> i32 {\n    2\n}\n")]);

    let result = merge_texts(&base, &ours, &theirs);

    assert!(!result.is_clean(), "a divergent line must not merge clean");
    assert!(result.conflicts.contains_key("lib.rs"), "expected lib.rs conflicted");
    assert!(!result.merged.contains_key("lib.rs"), "a conflicted file must not be merged");
}

/// Both sides merge cleanly at the text level yet the result is broken: one
/// removed a function, the other added a call to it. This is the class of bug
/// the crate exists to catch, and it must survive the set-wise entry point.
#[test]
fn a_clean_text_merge_that_breaks_a_reference_is_reported_as_hidden() {
    let base = map(&[
        ("lib.rs", "fn helper() -> i32 {\n    1\n}\n\nfn keep() -> i32 {\n    2\n}\n"),
        ("main.rs", "fn main() {\n    println!(\"hi\");\n}\n"),
    ]);
    let ours = map(&[
        ("lib.rs", "fn keep() -> i32 {\n    2\n}\n"),
        ("main.rs", "fn main() {\n    println!(\"hi\");\n}\n"),
    ]);
    let theirs = map(&[
        ("lib.rs", "fn helper() -> i32 {\n    1\n}\n\nfn keep() -> i32 {\n    2\n}\n"),
        ("main.rs", "fn main() {\n    println!(\"{}\", helper());\n}\n"),
    ]);

    let result = merge_texts(&base, &ours, &theirs);

    assert!(result.conflicts.is_empty(), "text layer should merge clean: {:?}", result.conflicts);
    assert!(
        result.hidden.contains("helper"),
        "dangling reference to `helper` not reported, hidden={:?}",
        result.hidden
    );
    assert!(!result.is_clean(), "a hidden conflict must not count as clean");
}

/// A file deleted on one side and untouched on the other is a deletion, not a
/// conflict and not an empty file.
#[test]
fn a_one_sided_delete_is_carried_through() {
    let base = map(&[
        ("gone.rs", "fn gone() {}\n"),
        ("stay.rs", "fn stay() {}\n"),
    ]);
    let mut ours = base.clone();
    ours.remove("gone.rs");
    let theirs = base.clone();

    let result = merge_texts(&base, &ours, &theirs);

    assert!(result.deleted.contains("gone.rs"), "deletion lost: {:?}", result.deleted);
    assert!(!result.merged.contains_key("gone.rs"), "deleted file must not be merged back");
    assert!(result.merged.contains_key("stay.rs"), "untouched file must survive");
}
