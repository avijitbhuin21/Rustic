//! Regression tests for the pre-integration review (R1-R6).
//!
//! Each test corresponds to one acceptance criterion. They exist to stop the
//! specific failure mode coming back, so they assert the *guarantee*, not the
//! current implementation's shape.

use lattice_merge::checks::{CallSite, Definition, Signature};
use lattice_merge::engine_b::{merge_texts, TextMap};
use lattice_merge::hidden::merge_file_checked_with;
use lattice_merge::{merge_file_checked, Arity, SymbolExtractor};
use std::collections::BTreeSet;

/// An extractor for a language with no bundled grammar, supplying only the two
/// required methods — exactly the shape a host reaches for first.
struct NamesOnlyGo;

impl SymbolExtractor for NamesOnlyGo {
    fn defined(&self, _lang: &str, text: &str) -> BTreeSet<String> {
        text.lines()
            .filter_map(|l| l.strip_prefix("func "))
            .filter_map(|l| l.split('(').next())
            .map(|n| n.trim().to_string())
            .collect()
    }

    fn referenced(&self, _lang: &str, _text: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

/// The full-capability version: what a host must implement to earn a clean
/// verdict on a language the crate has no grammar for.
struct FullGo;

impl SymbolExtractor for FullGo {
    fn defined(&self, lang: &str, text: &str) -> BTreeSet<String> {
        NamesOnlyGo.defined(lang, text)
    }

    fn referenced(&self, _lang: &str, _text: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn definitions(&self, _lang: &str, text: &str) -> Option<Vec<Definition>> {
        Some(
            text.lines()
                .filter_map(|l| l.strip_prefix("func "))
                .filter_map(|l| l.split('(').next())
                .map(|n| Definition {
                    scope: "file".into(),
                    kind: "func".into(),
                    name: n.trim().to_string(),
                })
                .collect(),
        )
    }

    fn signatures(&self, _lang: &str, text: &str) -> Option<Vec<Signature>> {
        Some(
            text.lines()
                .filter_map(|l| l.strip_prefix("func "))
                .filter_map(|l| {
                    let name = l.split('(').next()?.trim().to_string();
                    let params = l.split_once('(')?.1.split_once(')')?.0;
                    let count =
                        if params.trim().is_empty() { 0 } else { params.split(',').count() };
                    Some(Signature { name, arity: Arity { min: count, max: Some(count) } })
                })
                .collect(),
        )
    }

    fn call_sites(&self, _lang: &str, text: &str) -> Option<Vec<CallSite>> {
        Some(
            text.lines()
                .filter_map(|l| l.trim().strip_prefix("call "))
                .filter_map(|l| {
                    let name = l.split('(').next()?.trim().to_string();
                    let args = l.split_once('(')?.1.split_once(')')?.0;
                    let count = if args.trim().is_empty() { 0 } else { args.split(',').count() };
                    Some(CallSite { name, args: count })
                })
                .collect(),
        )
    }
}

/// R1: the headline false-safe. A duplicate definition in a language the
/// extractor cannot fully analyse must never be reported as clean.
#[test]
fn an_unanalysable_language_is_never_reported_clean() {
    let base = "func A() {\n}\n";
    let ours = "func A() {\n}\nfunc B() {\n}\n";
    let theirs = "func A() {\n}\nfunc B() {\n}\n";

    let r = merge_file_checked_with(&NamesOnlyGo, Some("go"), base, ours, theirs)
        .expect("merge runs");

    assert!(!r.is_conflicted(), "the text merge itself should succeed");
    assert!(
        !r.is_clean(),
        "a merge whose duplicate/arity checks never ran must not be clean; defects: {:?}",
        r.defects()
    );
    assert!(!r.coverage.is_complete());
    assert_eq!(r.coverage.missing(), vec!["duplicates", "arity"]);
    assert!(r.defects().iter().any(|d| d == "unchecked:duplicates"));
}

/// R1, second half: a host that supplies no coverage for a language must not be
/// able to launder a genuine duplicate past the checks.
///
/// The two `dup` bodies differ and are inserted at disjoint positions, so the
/// merge cannot collapse them the way it collapses an identical double add.
#[test]
fn a_real_duplicate_in_go_is_either_reported_or_marked_unchecked() {
    let base = "func A() {\n}\nfunc Z() {\n}\n";
    let ours = "func A() {\n}\nfunc dup() {\n  a\n}\nfunc Z() {\n}\n";
    let theirs = "func A() {\n}\nfunc Z() {\n}\nfunc dup() {\n  b\n}\n";

    let partial = merge_file_checked_with(&NamesOnlyGo, Some("go"), base, ours, theirs)
        .expect("merge runs");
    assert!(!partial.is_clean(), "must not bless an unchecked duplicate");

    let full =
        merge_file_checked_with(&FullGo, Some("go"), base, ours, theirs).expect("merge runs");
    assert!(!full.is_conflicted(), "disjoint inserts should compose: {}", full.outcome.text);
    assert!(full.coverage.is_complete(), "FullGo supplies every capability");
    assert!(
        full.duplicates.contains("func:dup"),
        "the duplicate should be caught via the host extractor, got {:?} for:\n{}",
        full.defects(),
        full.outcome.text
    );
    assert!(!full.is_clean());
}

/// R2: a host extractor must be able to surface an arity mismatch in a language
/// with no bundled grammar.
#[test]
fn a_host_extractor_detects_arity_mismatch_without_a_bundled_grammar() {
    let base = "func f(a) {\n}\ncall f(1)\n";
    let ours = "func f(a, b) {\n}\ncall f(1)\n";
    let theirs = "func f(a) {\n}\ncall f(1)\n";

    let r = merge_file_checked_with(&FullGo, Some("go"), base, ours, theirs).expect("merge runs");

    assert!(r.coverage.is_complete());
    assert!(
        r.arity.contains("f/1"),
        "a one-arg call against a two-param signature is a mismatch, got {:?}",
        r.defects()
    );
    assert!(!r.is_clean());
}

/// R2: a supported language keeps working through the same path — the bundled
/// extractor must report complete coverage and still catch a real defect.
#[test]
fn a_bundled_language_reports_complete_coverage() {
    let base = "fn f(a: i32) -> i32 { a }\nfn main() { f(1); }\n";
    let ours = "fn f(a: i32, b: i32) -> i32 { a + b }\nfn main() { f(1); }\n";
    let theirs = base;

    let r = merge_file_checked(Some("rust"), base, ours, theirs).expect("merge runs");
    assert!(r.coverage.is_complete(), "rust is bundled: {:?}", r.coverage);
    assert!(r.arity.contains("f/1"), "defects: {:?}", r.defects());
}

/// The `None` returned by an unimplemented optional method means *no opinion*,
/// not *unanalysable*: for a bundled language it must fall back to the grammar
/// rather than silently dropping coverage.
///
/// Without the fallback a host that implements only `defined`/`referenced` — the
/// documented minimum — would lose duplicate and arity checking on Rust, turning
/// an additive trait extension into a downgrade.
#[test]
fn a_partial_extractor_still_gets_bundled_grammar_coverage() {
    struct NamesOnly;
    impl SymbolExtractor for NamesOnly {
        fn defined(&self, _lang: &str, _text: &str) -> BTreeSet<String> {
            BTreeSet::new()
        }
        fn referenced(&self, _lang: &str, _text: &str) -> BTreeSet<String> {
            BTreeSet::new()
        }
    }

    let base = "fn a() {}\nfn z() {}\n";
    let ours = "fn a() {}\nfn dup() { let x = 1; }\nfn z() {}\n";
    let theirs = "fn a() {}\nfn z() {}\nfn dup() { let y = 2; }\n";

    let checked =
        merge_file_checked_with(&NamesOnly, Some("rust"), base, ours, theirs).expect("merge runs");

    assert!(
        checked.coverage.is_complete(),
        "rust is bundled, so coverage must not depend on the host: {:?}",
        checked.defects()
    );
    assert!(
        !checked.is_conflicted(),
        "disjoint inserts should compose: {}",
        checked.outcome.text
    );
    assert!(
        checked.duplicates.contains("function_item:dup"),
        "the grammar should still catch the duplicate, got {:?}",
        checked.defects()
    );
}

/// R3: `merge_texts` must be reproducible. Symbol ids are derived from content,
/// so repeated runs over identical input cannot permute anything keyed on them.
#[test]
fn merge_texts_is_byte_identical_across_runs() {
    let mut base = TextMap::new();
    let mut ours = TextMap::new();
    let mut theirs = TextMap::new();
    for i in 0..12 {
        base.insert(format!("m{i}.rs"), format!("fn a{i}() {{ 1 }}\nfn b{i}() {{ 2 }}\n"));
        ours.insert(format!("m{i}.rs"), format!("fn a{i}() {{ 10 }}\nfn b{i}() {{ 2 }}\n"));
        theirs.insert(format!("m{i}.rs"), format!("fn a{i}() {{ 1 }}\nfn b{i}() {{ 20 }}\n"));
    }

    let first = merge_texts(&base, &ours, &theirs);
    for _ in 0..40 {
        let again = merge_texts(&base, &ours, &theirs);
        assert_eq!(first.merged, again.merged, "merged text must be reproducible");
        assert_eq!(first.strategies, again.strategies, "strategies must be reproducible");
        assert_eq!(first.conflicts, again.conflicts);
        assert_eq!(first.structural, again.structural);
    }
}

/// R4: an empty `defects()` on a conflicted merge is not good news, and
/// `is_conflicted` is what distinguishes the two.
#[test]
fn a_conflicted_merge_is_distinguishable_from_a_clean_one() {
    let base = "fn f() -> i32 { 0 }\n";
    let ours = "fn f() -> i32 { 1 }\n";
    let theirs = "fn f() -> i32 { 2 }\n";

    let r = merge_file_checked(Some("rust"), base, ours, theirs).expect("merge runs");
    assert!(r.is_conflicted(), "divergent one-line edits conflict");
    assert!(!r.is_clean());
}

/// R6: a host must be able to tell which paths carry a structural guarantee.
#[test]
fn set_merge_reports_which_paths_were_structurally_checked() {
    let mut base = TextMap::new();
    let mut ours = TextMap::new();
    let mut theirs = TextMap::new();

    base.insert("a.rs".into(), "fn keep() -> i32 { 1 }\n".into());
    ours.insert("a.rs".into(), "fn keep() -> i32 { 2 }\n".into());
    theirs.insert("a.rs".into(), "fn keep() -> i32 { 1 }\n".into());

    base.insert("notes.go".into(), "func A() {\n}\n".into());
    ours.insert("notes.go".into(), "func A() {\n}\nfunc B() {\n}\n".into());
    theirs.insert("notes.go".into(), "func A() {\n}\n".into());

    let set = merge_texts(&base, &ours, &theirs);

    assert!(set.was_structurally_checked("a.rs"), "rust is covered");
    assert!(!set.was_structurally_checked("notes.go"), "go has no bundled grammar");
    assert_eq!(set.unverified(), BTreeSet::from(["notes.go".to_string()]));
    assert!(
        !set.is_clean(),
        "a set containing an unverifiable file cannot be blanket-clean"
    );
}

/// R6 complement: an all-supported set with no defects is still clean, so the
/// stricter `is_clean` has not made the happy path unreachable.
#[test]
fn an_all_supported_clean_set_is_still_clean() {
    let mut base = TextMap::new();
    let mut ours = TextMap::new();
    let mut theirs = TextMap::new();

    base.insert("a.rs".into(), "fn a() -> i32 { 1 }\n".into());
    ours.insert("a.rs".into(), "fn a() -> i32 { 2 }\n".into());
    theirs.insert("a.rs".into(), "fn a() -> i32 { 1 }\n".into());

    base.insert("b.rs".into(), "fn b() -> i32 { 1 }\n".into());
    ours.insert("b.rs".into(), "fn b() -> i32 { 1 }\n".into());
    theirs.insert("b.rs".into(), "fn b() -> i32 { 3 }\n".into());

    let set = merge_texts(&base, &ours, &theirs);
    assert!(set.unverified().is_empty(), "both files are rust");
    assert!(set.is_clean(), "checks: {:?} hidden: {:?}", set.checks, set.hidden);
}
