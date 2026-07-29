//! Property tests over generated three-way merges. The corpus could only
//! assert "never loses or corrupts content" on the cases it happened to
//! contain; these hold over every input the generators reach.

use lattice_merge::parsers::rough_tokens;
use lattice_merge::{merge_file, MergeStatus};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// Vocabulary the engine itself may emit around a conflict, which is notation
/// rather than content.
const MARKER_VOCAB: &[&str] = &["<", ">", "=", "|", "base", "ours", "theirs"];

/// What one side did to one declaration. Each rendering carries a token unique
/// to its declaration, so a containment check cannot pass by coincidence.
#[derive(Clone, Debug, PartialEq)]
enum Edit {
    Keep,
    Body(u32),
    Delete,
}

impl Edit {
    /// This declaration's rendering, or None when the side deleted it.
    fn render(&self, i: usize) -> Option<String> {
        let body = match self {
            Edit::Delete => return None,
            Edit::Keep => format!("V{i}b"),
            Edit::Body(v) => format!("V{i}e{v}"),
        };
        Some(format!("fn f{i}() -> u32 {{\n    {body}\n}}\n"))
    }
}

/// Renders a whole file from per-declaration edits.
fn render(edits: &[Edit]) -> String {
    let mut out = String::new();
    for (i, edit) in edits.iter().enumerate() {
        if let Some(text) = edit.render(i) {
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        6 => Just(Edit::Keep),
        3 => (1u32..4).prop_map(Edit::Body),
        1 => Just(Edit::Delete),
    ]
}

/// Two independently edited revisions of the same generated base.
fn revisions() -> impl Strategy<Value = (Vec<Edit>, Vec<Edit>)> {
    (3usize..9).prop_flat_map(|n| {
        (
            proptest::collection::vec(edit(), n),
            proptest::collection::vec(edit(), n),
        )
    })
}

/// The unedited revision of `n` declarations.
fn base_of(n: usize) -> Vec<Edit> {
    vec![Edit::Keep; n]
}

/// Every distinct token in a text.
fn tokens(text: &str) -> BTreeSet<String> {
    rough_tokens(text).into_iter().collect()
}

proptest! {
    /// A merge may conflict, but it may never put content in the result that
    /// none of the three inputs contained — invented content is corruption no
    /// downstream check can catch.
    #[test]
    fn a_merge_never_invents_content((left, right) in revisions()) {
        let base = render(&base_of(left.len()));
        let left_text = render(&left);
        let right_text = render(&right);
        let outcome = merge_file(Some("rust"), &base, &left_text, &right_text).unwrap();

        let mut allowed = tokens(&base);
        allowed.extend(tokens(&left_text));
        allowed.extend(tokens(&right_text));
        allowed.extend(MARKER_VOCAB.iter().map(|s| (*s).to_string()));
        let invented: Vec<String> =
            tokens(&outcome.text).difference(&allowed).cloned().collect();
        prop_assert!(invented.is_empty(), "invented {invented:?} in\n{}", outcome.text);
    }

    /// A clean merge must carry through every change only one side made, and
    /// keep what neither side touched. Declarations both sides changed
    /// differently are the engine's call and are not asserted here.
    #[test]
    fn a_clean_merge_never_loses_a_one_sided_change((left, right) in revisions()) {
        let n = left.len();
        let base = render(&base_of(n));
        let outcome = merge_file(Some("rust"), &base, &render(&left), &render(&right)).unwrap();
        prop_assume!(outcome.status == MergeStatus::Clean);

        for i in 0..n {
            let expected = match (&left[i], &right[i]) {
                (a, b) if a == b => a.clone(),
                (Edit::Keep, other) | (other, Edit::Keep) => other.clone(),
                _ => continue,
            };
            match expected.render(i) {
                Some(text) => prop_assert!(
                    outcome.text.contains(text.trim_end()),
                    "f{i} should read {expected:?} in\n{}",
                    outcome.text
                ),
                None => prop_assert!(
                    !outcome.text.contains(&format!("fn f{i}()")),
                    "f{i} was deleted on one side, untouched on the other, yet survives in\n{}",
                    outcome.text
                ),
            }
        }
    }

    /// When one side made no change at all, the merge is that side's identity:
    /// the other side's revision, byte for byte.
    #[test]
    fn an_unchanged_side_is_an_identity((left, _right) in revisions()) {
        let base = render(&base_of(left.len()));
        let left_text = render(&left);
        let same = merge_file(Some("rust"), &base, &left_text, &base).unwrap();
        prop_assert_eq!(same.status, MergeStatus::Clean);
        prop_assert_eq!(&same.text, &left_text);
        let flipped = merge_file(Some("rust"), &base, &base, &left_text).unwrap();
        prop_assert_eq!(flipped.status, MergeStatus::Clean);
        prop_assert_eq!(&flipped.text, &left_text);
    }
}
