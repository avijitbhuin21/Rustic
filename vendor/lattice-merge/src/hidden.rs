//! Hidden-conflict detection: names that existed in the base, are gone from
//! the merged state, and are still referenced by it. A text merge calls this
//! clean; it is the class of silent breakage that concurrent agents produce.
//!
//! Detection is name-based — no type information, no arity or signature
//! awareness, no cross-language references — so the extractor interface is
//! deliberately small and a host with its own symbol index can supply one.

use crate::checks::{
    arity_mismatches_in, duplicates_in, tree_call_sites, tree_definitions, tree_signatures,
    CallSite, Definition, Signature,
};
use crate::parsers::{defined_names, referenced_names, supported};
use crate::structural_merge::{merge_file, MergeOutcome, MergeStatus};
use anyhow::Result;
use std::collections::BTreeSet;

/// Supplies the defined and referenced names of one source text.
///
/// `defined` and `referenced` back hidden-conflict detection and are required.
/// The other three back the duplicate and arity checks and are optional: the
/// default implementations return `None`, meaning *no opinion*. A `None` falls
/// back to the bundled grammar for `lang`, so an extractor that only implements
/// the two required methods still gets full duplicate and arity coverage for the
/// five bundled languages.
///
/// Coverage is only reported missing when the extractor **and** the bundled
/// grammars both decline — which is exactly the case a host adding its own
/// languages needs to know about. See [`CheckCoverage`].
///
/// Return `None`, not `Some(vec![])`, when you cannot analyse a language:
/// an empty vector means *analysed, found nothing* and will be trusted.
pub trait SymbolExtractor {
    /// Names this text declares.
    fn defined(&self, lang: &str, text: &str) -> BTreeSet<String>;

    /// Names this text references without declaring them itself.
    fn referenced(&self, lang: &str, text: &str) -> BTreeSet<String>;

    /// Whether `defined` and `referenced` are meaningful for `lang`. An
    /// extractor that returns empty sets for a language it does not understand
    /// must override this, or its silence will be read as "no conflicts".
    fn supports(&self, _lang: &str) -> bool {
        true
    }

    /// Declarations subject to the duplicate rule, or `None` if unsupported.
    fn definitions(&self, _lang: &str, _text: &str) -> Option<Vec<Definition>> {
        None
    }

    /// Callable signatures, or `None` if unsupported.
    fn signatures(&self, _lang: &str, _text: &str) -> Option<Vec<Signature>> {
        None
    }

    /// Call sites with positional argument counts, or `None` if unsupported.
    fn call_sites(&self, _lang: &str, _text: &str) -> Option<Vec<CallSite>> {
        None
    }
}

/// The built-in tree-sitter extractor covering the bundled grammars.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeSitterExtractor;

impl SymbolExtractor for TreeSitterExtractor {
    fn defined(&self, lang: &str, text: &str) -> BTreeSet<String> {
        defined_names(lang, text)
    }

    fn referenced(&self, lang: &str, text: &str) -> BTreeSet<String> {
        referenced_names(lang, text)
    }

    fn supports(&self, lang: &str) -> bool {
        supported(lang)
    }

    fn definitions(&self, lang: &str, text: &str) -> Option<Vec<Definition>> {
        tree_definitions(lang, text)
    }

    fn signatures(&self, lang: &str, text: &str) -> Option<Vec<Signature>> {
        tree_signatures(lang, text)
    }

    fn call_sites(&self, lang: &str, text: &str) -> Option<Vec<CallSite>> {
        tree_call_sites(lang, text)
    }
}

/// Which of the three checks actually ran.
///
/// A check that could not run contributes no defects, which is indistinguishable
/// from a check that ran and found none. `CheckedMerge::is_clean` therefore
/// requires full coverage: an unanalysable language is never reported as safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckCoverage {
    pub hidden: bool,
    pub duplicates: bool,
    pub arity: bool,
}

impl CheckCoverage {
    /// Every check ran.
    pub fn complete() -> Self {
        CheckCoverage {
            hidden: true,
            duplicates: true,
            arity: true,
        }
    }

    /// No check ran.
    pub fn none() -> Self {
        CheckCoverage {
            hidden: false,
            duplicates: false,
            arity: false,
        }
    }

    /// True when all three checks ran and their results can be trusted.
    pub fn is_complete(&self) -> bool {
        self.hidden && self.duplicates && self.arity
    }

    /// The checks that did not run, for reporting.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.hidden {
            out.push("hidden");
        }
        if !self.duplicates {
            out.push("duplicates");
        }
        if !self.arity {
            out.push("arity");
        }
        out
    }
}

/// Names defined in `base_files`, absent from `merged_files`, yet still
/// referenced there — using a caller-supplied extractor.
pub fn hidden_conflicts_with(
    extractor: &dyn SymbolExtractor,
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> BTreeSet<String> {
    let mut base_defined = BTreeSet::new();
    for text in base_files {
        base_defined.extend(extractor.defined(lang, text));
    }
    let mut merged_defined = BTreeSet::new();
    let mut merged_refs = BTreeSet::new();
    for text in merged_files {
        merged_defined.extend(extractor.defined(lang, text));
        merged_refs.extend(extractor.referenced(lang, text));
    }
    base_defined
        .difference(&merged_defined)
        .filter(|name| merged_refs.contains(*name))
        .cloned()
        .collect()
}

/// Names defined in `base_files`, absent from `merged_files`, yet still
/// referenced there — using the built-in tree-sitter extractor.
pub fn hidden_conflicts(
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> BTreeSet<String> {
    hidden_conflicts_with(&TreeSitterExtractor, lang, base_files, merged_files)
}

/// A merge outcome plus every structural defect its result introduces.
///
/// Empty defect sets are only meaningful alongside `outcome.status == Clean`
/// **and** `coverage.is_complete()`. Checks do not run on a conflicted merge
/// (it already needs resolution) and cannot run on a language no extractor
/// understands, so `defects()` being empty is not by itself good news — use
/// [`CheckedMerge::is_clean`].
#[derive(Clone, Debug)]
pub struct CheckedMerge {
    pub outcome: MergeOutcome,
    pub hidden: BTreeSet<String>,
    /// Names the merge defines twice in one scope (`kind:name`).
    pub duplicates: BTreeSet<String>,
    /// Call sites the merge leaves at an argument count no signature accepts
    /// (`name/argc`).
    pub arity: BTreeSet<String>,
    /// Which checks actually ran. See [`CheckCoverage`].
    pub coverage: CheckCoverage,
}

impl CheckedMerge {
    /// True when the merge is textually clean, every check ran, and none of
    /// them found a dangling reference, duplicate definition or stale-arity
    /// call site — the only outcome safe to accept unattended.
    pub fn is_clean(&self) -> bool {
        self.outcome.status == MergeStatus::Clean
            && self.coverage.is_complete()
            && self.hidden.is_empty()
            && self.duplicates.is_empty()
            && self.arity.is_empty()
    }

    /// True when the text merge itself failed and the result carries conflict
    /// markers. Distinct from `!is_clean()`, which is also true for a textually
    /// clean merge that a check rejected or could not verify.
    pub fn is_conflicted(&self) -> bool {
        self.outcome.status != MergeStatus::Clean
    }

    /// Every defect as flat labels, for reporting. Includes `unchecked:<name>`
    /// for each check that could not run.
    pub fn defects(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .hidden
            .iter()
            .map(|n| format!("dangling:{n}"))
            .collect();
        out.extend(self.duplicates.iter().map(|n| format!("duplicate:{n}")));
        out.extend(self.arity.iter().map(|n| format!("arity:{n}")));
        out.extend(
            self.coverage
                .missing()
                .iter()
                .map(|c| format!("unchecked:{c}")),
        );
        out
    }
}

/// Duplicate definitions the merged files introduce, and whether the extractor
/// could analyse every file. Per-file by construction: the same name in two
/// different files is a different scope and perfectly legal.
fn new_duplicates_with(
    extractor: &dyn SymbolExtractor,
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> (BTreeSet<String>, bool) {
    let mut ok = true;
    let mut base = BTreeSet::new();
    for text in base_files {
        match extractor
            .definitions(lang, text)
            .or_else(|| tree_definitions(lang, text))
        {
            Some(defs) => base.extend(duplicates_in(&defs)),
            None => ok = false,
        }
    }
    let mut merged = BTreeSet::new();
    for text in merged_files {
        match extractor
            .definitions(lang, text)
            .or_else(|| tree_definitions(lang, text))
        {
            Some(defs) => merged.extend(duplicates_in(&defs)),
            None => ok = false,
        }
    }
    (merged.difference(&base).cloned().collect(), ok)
}

/// Arity mismatches across a pooled file group, and whether the extractor could
/// analyse every file.
fn arity_of(
    extractor: &dyn SymbolExtractor,
    lang: &str,
    texts: &[&str],
) -> (BTreeSet<String>, bool) {
    let mut sigs = Vec::new();
    let mut calls = Vec::new();
    let mut ok = true;
    for text in texts {
        match extractor
            .signatures(lang, text)
            .or_else(|| tree_signatures(lang, text))
        {
            Some(found) => sigs.extend(found),
            None => ok = false,
        }
        match extractor
            .call_sites(lang, text)
            .or_else(|| tree_call_sites(lang, text))
        {
            Some(found) => calls.extend(found),
            None => ok = false,
        }
    }
    (arity_mismatches_in(&sigs, &calls), ok)
}

/// Arity mismatches the merged state introduces, and whether every file could
/// be analysed.
fn new_arity_with(
    extractor: &dyn SymbolExtractor,
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> (BTreeSet<String>, bool) {
    let (base, base_ok) = arity_of(extractor, lang, base_files);
    let (merged, merged_ok) = arity_of(extractor, lang, merged_files);
    (
        merged.difference(&base).cloned().collect(),
        base_ok && merged_ok,
    )
}

/// All three checks, run with whatever capabilities `extractor` offers.
fn run_checks(
    extractor: &dyn SymbolExtractor,
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> (
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeSet<String>,
    CheckCoverage,
) {
    let hidden_ok = extractor.supports(lang);
    let hidden = if hidden_ok {
        hidden_conflicts_with(extractor, lang, base_files, merged_files)
    } else {
        BTreeSet::new()
    };
    let (duplicates, duplicates_ok) =
        new_duplicates_with(extractor, lang, base_files, merged_files);
    let (arity, arity_ok) = new_arity_with(extractor, lang, base_files, merged_files);
    (
        hidden,
        duplicates,
        arity,
        CheckCoverage {
            hidden: hidden_ok,
            duplicates: duplicates_ok,
            arity: arity_ok,
        },
    )
}

/// Three-way merge that also reports hidden conflicts, with a caller-supplied
/// extractor. Checks are only computed for a textually clean merge: a
/// conflicted merge already needs human or agent resolution.
pub fn merge_file_checked_with(
    extractor: &dyn SymbolExtractor,
    lang: Option<&str>,
    base: &str,
    left: &str,
    right: &str,
) -> Result<CheckedMerge> {
    let outcome = merge_file(lang, base, left, right)?;
    let mut checked = CheckedMerge {
        outcome,
        hidden: BTreeSet::new(),
        duplicates: BTreeSet::new(),
        arity: BTreeSet::new(),
        coverage: CheckCoverage::none(),
    };
    if let Some(lang) = lang {
        if checked.outcome.status == MergeStatus::Clean {
            let merged: &str = &checked.outcome.text;
            let (hidden, duplicates, arity, coverage) =
                run_checks(extractor, lang, &[base], &[merged]);
            checked.hidden = hidden;
            checked.duplicates = duplicates;
            checked.arity = arity;
            checked.coverage = coverage;
        }
    }
    Ok(checked)
}

/// Three-way merge over a whole file set, checked across files: dangling
/// references and stale-arity call sites are pooled over every merged file, so
/// a signature change in one file and a call in another are compared. A single
/// conflicting file escalates the whole set.
pub fn merge_files_checked(
    lang: &str,
    files: &[(String, String, String, String)],
) -> Result<(Vec<(String, MergeOutcome)>, CheckedMerge)> {
    let mut per_file = Vec::new();
    let mut conflicted = false;
    let mut strategies: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    let mut merged_texts = Vec::new();
    let mut base_texts = Vec::new();
    for (path, base, left, right) in files {
        let outcome = merge_file(Some(lang), base, left, right)?;
        conflicted |= outcome.status != MergeStatus::Clean;
        for (k, v) in &outcome.strategies {
            *strategies.entry(k.clone()).or_insert(0) += v;
        }
        base_texts.push(base.clone());
        merged_texts.push(outcome.text.clone());
        per_file.push((path.clone(), outcome));
    }
    let joined = merged_texts.join("\n");
    let outcome = MergeOutcome {
        status: if conflicted {
            MergeStatus::Conflict
        } else {
            MergeStatus::Clean
        },
        text: joined,
        strategies,
    };
    let mut checked = CheckedMerge {
        outcome,
        hidden: BTreeSet::new(),
        duplicates: BTreeSet::new(),
        arity: BTreeSet::new(),
        coverage: CheckCoverage::none(),
    };
    if !conflicted {
        let base_refs: Vec<&str> = base_texts.iter().map(String::as_str).collect();
        let merged_refs: Vec<&str> = merged_texts.iter().map(String::as_str).collect();
        let (hidden, duplicates, arity, coverage) =
            run_checks(&TreeSitterExtractor, lang, &base_refs, &merged_refs);
        checked.hidden = hidden;
        checked.duplicates = duplicates;
        checked.arity = arity;
        checked.coverage = coverage;
    }
    Ok((per_file, checked))
}

/// Criss-cross safety: with more than one candidate merge base, merge against
/// each and accept a clean result only when every base agrees on it. Divergent
/// clean results mean the answer depends on which base was picked, which is
/// exactly the silent breakage a single-base merge cannot see.
pub fn merge_file_checked_multibase(
    lang: Option<&str>,
    bases: &[&str],
    left: &str,
    right: &str,
) -> Result<CheckedMerge> {
    let mut first: Option<CheckedMerge> = None;
    for base in bases {
        let checked = merge_file_checked(lang, base, left, right)?;
        match &first {
            None => first = Some(checked),
            Some(prev) => {
                if !prev.is_clean()
                    || !checked.is_clean()
                    || prev.outcome.text != checked.outcome.text
                {
                    let mut out = prev.clone();
                    out.outcome.status = MergeStatus::Conflict;
                    out.outcome
                        .strategies
                        .insert("multibase_disagreement".into(), 1);
                    return Ok(out);
                }
            }
        }
    }
    first.ok_or_else(|| anyhow::anyhow!("at least one merge base is required"))
}

/// Three-way merge that also reports hidden conflicts, using the built-in
/// tree-sitter extractor.
pub fn merge_file_checked(
    lang: Option<&str>,
    base: &str,
    left: &str,
    right: &str,
) -> Result<CheckedMerge> {
    merge_file_checked_with(&TreeSitterExtractor, lang, base, left, right)
}
