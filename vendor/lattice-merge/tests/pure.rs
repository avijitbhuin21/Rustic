//! `lattice-merge` used exactly as a host would: pure functions over strings,
//! no repository, no `.lat/` directory, no store.

use lattice_merge::hidden::{merge_file_checked, merge_file_checked_with, SymbolExtractor};
use lattice_merge::linemerge::{line_merge, parse_markers, Segment};
use lattice_merge::{hidden_conflicts, merge_file, MergeStatus};
use std::collections::BTreeSet;

/// A final line without a newline must not have the next conflict marker
/// appended to it: `parse_markers` only sees a marker that starts a line, so a
/// glued `}=======` silently degrades the structural merge to a raw conflict.
#[test]
fn conflict_markers_start_a_line_when_an_input_lacks_a_final_newline() {
    let base = "const a = 1;\n\nfunction x() {\n  return 0;\n}";
    let left = "const a = 1;\n\nfunction x() {\n  return 1;\n}\n";
    let right = "const a = 1;\n\nfunction x() {\n  return 2;\n}\n";

    let (conflicts, text) = line_merge(base, left, right).expect("merge runs");
    assert!(conflicts >= 1, "expected a conflict, got:\n{text}");

    for marker in ["<<<<<<<", "|||||||", "=======", ">>>>>>>"] {
        let mut from = 0;
        let mut seen = false;
        while let Some(offset) = text[from..].find(marker) {
            let at = from + offset;
            assert!(
                at == 0 || text.as_bytes()[at - 1] == b'\n',
                "{marker} does not start a line in:\n{text}"
            );
            seen = true;
            from = at + marker.len();
        }
        assert!(seen, "{marker} missing from:\n{text}");
    }

    let conflicted = parse_markers(&text)
        .into_iter()
        .filter(|s| matches!(s, Segment::Conflict(_)))
        .count();
    assert_eq!(conflicted, 1, "parse_markers must see the hunk in:\n{text}");
}


/// Base/left/right triple where left deletes `helper` and right adds a
/// reference to it — the T4 case that a text merge resolves silently.
struct Case {
    lang: &'static str,
    base: &'static str,
    left: &'static str,
    right: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            lang: "rust",
            base: "fn helper() -> u32 { 1 }\n\nfn filler_a() -> u32 { 2 }\n\nfn filler_b() -> u32 { 3 }\n\nfn caller() -> u32 { 0 }\n",
            left: "fn filler_a() -> u32 { 2 }\n\nfn filler_b() -> u32 { 3 }\n\nfn caller() -> u32 { 0 }\n",
            right: "fn helper() -> u32 { 1 }\n\nfn filler_a() -> u32 { 2 }\n\nfn filler_b() -> u32 { 3 }\n\nfn caller() -> u32 { helper() }\n",
        },
        Case {
            lang: "python",
            base: "def helper():\n    return 1\n\n\ndef filler_a():\n    return 2\n\n\ndef filler_b():\n    return 3\n\n\ndef caller():\n    return 0\n",
            left: "def filler_a():\n    return 2\n\n\ndef filler_b():\n    return 3\n\n\ndef caller():\n    return 0\n",
            right: "def helper():\n    return 1\n\n\ndef filler_a():\n    return 2\n\n\ndef filler_b():\n    return 3\n\n\ndef caller():\n    return helper()\n",
        },
        Case {
            lang: "javascript",
            base: "function helper() { return 1; }\n\nfunction fillerA() { return 2; }\n\nfunction fillerB() { return 3; }\n\nfunction caller() { return 0; }\n",
            left: "function fillerA() { return 2; }\n\nfunction fillerB() { return 3; }\n\nfunction caller() { return 0; }\n",
            right: "function helper() { return 1; }\n\nfunction fillerA() { return 2; }\n\nfunction fillerB() { return 3; }\n\nfunction caller() { return helper(); }\n",
        },
        Case {
            lang: "typescript",
            base: "function helper(): number { return 1; }\n\nfunction fillerA(): number { return 2; }\n\nfunction fillerB(): number { return 3; }\n\nfunction caller(): number { return 0; }\n",
            left: "function fillerA(): number { return 2; }\n\nfunction fillerB(): number { return 3; }\n\nfunction caller(): number { return 0; }\n",
            right: "function helper(): number { return 1; }\n\nfunction fillerA(): number { return 2; }\n\nfunction fillerB(): number { return 3; }\n\nfunction caller(): number { return helper(); }\n",
        },
        Case {
            lang: "tsx",
            base: "function helper(): number { return 1; }\n\nfunction fillerA(): number { return 2; }\n\nfunction fillerB(): number { return 3; }\n\nfunction caller(): number { return 0; }\n",
            left: "function fillerA(): number { return 2; }\n\nfunction fillerB(): number { return 3; }\n\nfunction caller(): number { return 0; }\n",
            right: "function helper(): number { return 1; }\n\nfunction fillerA(): number { return 2; }\n\nfunction fillerB(): number { return 3; }\n\nfunction caller(): number { return helper(); }\n",
        },
    ]
}

#[test]
fn merge_file_needs_no_repository() {
    let base = "fn alpha() { one(); }\n\nfn beta() { two(); }\n";
    let left = "fn alpha() { one_changed(); }\n\nfn beta() { two(); }\n";
    let right = "fn alpha() { one(); }\n\nfn beta() { two_changed(); }\n";
    let outcome = merge_file(Some("rust"), base, left, right).unwrap();
    assert_eq!(outcome.status, MergeStatus::Clean, "{:?}", outcome.strategies);
    assert!(outcome.text.contains("one_changed") && outcome.text.contains("two_changed"));
}

#[test]
fn both_sides_adding_the_same_declaration_is_not_duplicated() {
    let base = "fn alpha() { one(); }\n";
    let added = "\nfn shared() -> u32 { 7 }\n";
    let left = format!("{base}{added}");
    let extra = "\nfn only_theirs() -> u32 { 8 }\n";
    let right = format!("{base}{added}{extra}");

    let outcome = merge_file(Some("rust"), base, &left, &right).unwrap();
    assert_eq!(outcome.status, MergeStatus::Clean, "{:?}", outcome.strategies);
    assert_eq!(
        outcome.text.matches("fn shared").count(),
        1,
        "the shared declaration must appear once, not once per side: {}",
        outcome.text
    );
    assert!(outcome.text.contains("only_theirs"), "{}", outcome.text);
    assert!(
        outcome.strategies.contains_key("insert_dedup"),
        "the dedup guard should account for itself: {:?}",
        outcome.strategies
    );
}

#[test]
fn distinct_insertions_in_a_commutative_container_still_union() {
    let base = "fn alpha() { one(); }\n";
    let left = format!("{base}\nfn ours() -> u32 {{ 1 }}\n");
    let right = format!("{base}\nfn theirs() -> u32 {{ 2 }}\n");
    let outcome = merge_file(Some("rust"), base, &left, &right).unwrap();
    assert_eq!(outcome.status, MergeStatus::Clean, "{:?}", outcome.strategies);
    assert!(
        outcome.text.contains("fn ours") && outcome.text.contains("fn theirs"),
        "{}",
        outcome.text
    );
    assert!(!outcome.strategies.contains_key("insert_dedup"), "{:?}", outcome.strategies);
}

#[test]
fn delete_versus_new_reference_is_hidden_in_every_supported_language() {
    for case in cases() {
        let checked = merge_file_checked(Some(case.lang), case.base, case.left, case.right).unwrap();
        assert_eq!(
            checked.outcome.status,
            MergeStatus::Clean,
            "{} should merge cleanly as text: {:?}",
            case.lang,
            checked.outcome.strategies
        );
        assert!(
            checked.hidden.contains("helper"),
            "{} hidden conflict must be detected: {:?}",
            case.lang,
            checked.hidden
        );
        assert!(!checked.is_clean(), "{} must not be reported as safe", case.lang);
    }
}

#[test]
fn an_unrelated_edit_pair_has_no_hidden_conflict() {
    for case in cases() {
        let checked =
            merge_file_checked(Some(case.lang), case.base, case.base, case.right).unwrap();
        assert!(
            checked.is_clean(),
            "{} control case must stay clean: {:?}",
            case.lang,
            checked.hidden
        );
    }
}

#[test]
fn multi_file_hidden_conflicts_span_the_merged_set() {
    let base_a = "fn helper() -> u32 { 1 }\n";
    let base_b = "fn caller() -> u32 { 0 }\n";
    let merged_b = "fn caller() -> u32 { helper() }\n";
    let hidden = hidden_conflicts("rust", &[base_a, base_b], &[merged_b]);
    assert_eq!(hidden, BTreeSet::from(["helper".to_string()]));
}

struct HostIndex {
    defined: BTreeSet<String>,
}

impl SymbolExtractor for HostIndex {
    fn defined(&self, _lang: &str, _text: &str) -> BTreeSet<String> {
        self.defined.clone()
    }

    fn referenced(&self, _lang: &str, _text: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

#[test]
fn a_host_can_substitute_its_own_symbol_index() {
    let case = &cases()[0];
    let index = HostIndex { defined: BTreeSet::new() };
    let checked =
        merge_file_checked_with(&index, Some(case.lang), case.base, case.left, case.right).unwrap();
    assert!(
        checked.hidden.is_empty(),
        "the supplied extractor decides, not the built-in one: {:?}",
        checked.hidden
    );
    assert!(checked.is_clean());
}
