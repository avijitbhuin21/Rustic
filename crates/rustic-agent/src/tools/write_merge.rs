//! Three-way merge at the write choke point.
//!
//! `lattice-merge` (MIT OR Apache-2.0, pure in-process) does the merge and the
//! structural checks. The merge base is the task's opening `file_history`
//! snapshot; "ours" is the agent's intent replayed against that base; "theirs"
//! is whatever is on disk right now.
//!
//! A merge is accepted only when it is clean AND every structural check
//! actually ran. `defects().is_empty()` alone does not mean clean — the checks
//! only run on textually-clean merges — so status is checked separately, which
//! `is_clean()` does. Anything else becomes a briefing for a resolver.

/// Above this size structural analysis is skipped entirely and the conflict is
/// escalated. Keeps a minified bundle or a generated file from burning the
/// write budget on a merge nobody can review.
const MAX_MERGE_BYTES: usize = 512 * 1024;

pub(crate) struct MergeAttempt {
    /// Merged text. Carries diff3 conflict markers when `clean` is false.
    pub text: String,
    pub status: &'static str,
    pub defects: Vec<String>,
    /// Safe to write without review: textually clean, every check ran, no
    /// defects found.
    pub clean: bool,
}

/// Merge `ours` and `theirs` over `base`. `None` means no merge was attempted
/// (file too large) and the caller must escalate.
pub(crate) fn try_merge(
    display_path: &str,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Option<MergeAttempt> {
    if base.len() > MAX_MERGE_BYTES
        || ours.len() > MAX_MERGE_BYTES
        || theirs.len() > MAX_MERGE_BYTES
    {
        return None;
    }

    // Rustic's tree-sitter coverage (19 languages) is wider than lattice's
    // built-in parsers (5), and hidden-conflict detection is name-based, so the
    // index-backed extractor is used for every language Rustic can parse. A
    // lang lattice cannot parse still gets a line merge plus the dangling-
    // reference check; `None` disables structural analysis entirely and is the
    // fallback for extensions neither side knows.
    let lang = rustic_treesitter::detect::language_for_path(std::path::Path::new(display_path))
        .or_else(|| lattice_merge::lang_for_path(display_path));
    let checked = lattice_merge::hidden::merge_file_checked_with(
        &super::write_extractor::IndexExtractor,
        lang,
        base,
        ours,
        theirs,
    )
    .ok()?;

    let status = match checked.outcome.status {
        lattice_merge::MergeStatus::Clean => "clean",
        lattice_merge::MergeStatus::Conflict => "conflict",
        lattice_merge::MergeStatus::ParseFallback => "parse_fallback",
    };

    Some(MergeAttempt {
        text: checked.outcome.text.clone(),
        status,
        defects: checked.defects(),
        clean: checked.is_clean(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_edits_merge_clean() {
        let base = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n";
        let ours = "fn a() {\n    111\n}\n\nfn b() {\n    2\n}\n";
        let theirs = "fn a() {\n    1\n}\n\nfn b() {\n    222\n}\n";
        let m = try_merge("src/lib.rs", base, ours, theirs).expect("merge attempted");
        assert!(m.clean, "status={} defects={:?}", m.status, m.defects);
        assert!(m.text.contains("111"));
        assert!(m.text.contains("222"));
    }

    #[test]
    fn same_region_edits_conflict_and_are_not_clean() {
        let base = "fn a() {\n    1\n}\n";
        let ours = "fn a() {\n    111\n}\n";
        let theirs = "fn a() {\n    222\n}\n";
        let m = try_merge("src/lib.rs", base, ours, theirs).expect("merge attempted");
        assert!(!m.clean);
    }

    #[test]
    fn oversized_input_is_not_merged() {
        let big = "x".repeat(MAX_MERGE_BYTES + 1);
        assert!(try_merge("src/lib.rs", &big, &big, &big).is_none());
    }

    #[test]
    fn hidden_conflict_blocks_a_textually_clean_merge() {
        // Ours deletes `helper` and its only call; theirs adds a new call to
        // `helper`. Textually disjoint, so line merge is happy — but the merged
        // file calls a function nothing defines.
        let base = "fn helper(x: i32) -> i32 {\n    x\n}\n\nfn one() {\n    helper(1);\n}\n\nfn two() {\n    let _ = 2;\n}\n";
        let ours = "fn one() {\n}\n\nfn two() {\n    let _ = 2;\n}\n";
        let theirs = "fn helper(x: i32) -> i32 {\n    x\n}\n\nfn one() {\n    helper(1);\n}\n\nfn two() {\n    let _ = helper(3);\n}\n";
        let m = try_merge("src/lib.rs", base, ours, theirs).expect("merge attempted");
        assert!(!m.clean, "hidden conflict must not auto-land");
        assert!(
            m.defects.iter().any(|d| d.contains("helper")),
            "defects={:?}",
            m.defects
        );
    }

    /// Go is the proof that the index-backed extractor is doing the work:
    /// lattice has no Go parser, so without it this merge looks clean.
    #[test]
    fn hidden_conflict_is_detected_in_a_language_lattice_cannot_parse() {
        let base = "package main\n\nfunc helper() int {\n\treturn 1\n}\n\nfunc one() int {\n\treturn helper()\n}\n\nfunc two() int {\n\treturn 2\n}\n";
        let ours =
            "package main\n\nfunc one() int {\n\treturn 0\n}\n\nfunc two() int {\n\treturn 2\n}\n";
        let theirs = "package main\n\nfunc helper() int {\n\treturn 1\n}\n\nfunc one() int {\n\treturn helper()\n}\n\nfunc two() int {\n\treturn helper() + 2\n}\n";
        let m = try_merge("cmd/main.go", base, ours, theirs).expect("merge attempted");
        assert!(!m.clean, "dangling `helper` must block the merge");
        assert!(
            m.defects.iter().any(|d| d.contains("helper")),
            "defects={:?}",
            m.defects
        );
    }
}
