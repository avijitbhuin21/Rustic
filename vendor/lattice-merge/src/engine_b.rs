//! Engine B: identity-aware merge orchestration.
//!
//! Layer 2 of the merge stack keyed by *stable symbol identity* rather than by
//! name or by line. `Alignment` builds a shared base identity frame and the
//! typed ops each side performed against it; that is what lets a rename on one
//! side compose with a body edit on the other instead of conflicting.
//!
//! Everything here is pure: file maps in, merged text out. No repository, no
//! object store, no `.lat/` directory. Hosts that keep their own storage can
//! call `merge_texts` directly.

use crate::algebra::commute::commute;
use crate::algebra::model::{Decl, State};
use crate::algebra::Op;
use crate::checks::{new_arity_mismatches, new_duplicate_definitions};
use crate::diff_ops::diff_to_ops;
use crate::hidden::hidden_conflicts;
use crate::parsers::{lang_for_path, parses_clean, rough_tokens};
use crate::structural_merge::{
    extract_named_chunk, merge_file_idaware, replace_named_chunk, MergeStatus,
};
use crate::symbols::{extract_decls, SymbolTable};
use std::collections::{BTreeMap, BTreeSet};

/// The languages the structural layer understands.
pub const LANGS: &[&str] = &["rust", "typescript", "tsx", "python", "javascript"];

/// A map of path -> file text. The unit both sides of a set-wise merge use.
pub type TextMap = BTreeMap<String, String>;

/// Build the algebra `State` and symbol table for the base tree — the shared
/// identity frame both sides' ops are computed against.
pub fn base_model(base_texts: &TextMap) -> (State, SymbolTable) {
    let mut extracted = BTreeMap::new();
    for (path, text) in base_texts {
        let name = path.rsplit('/').next().unwrap_or(path);
        extracted.insert(path.clone(), extract_decls(name, text));
    }
    let mut table = SymbolTable::default();
    table.reconcile(&extracted);
    let mut decls = BTreeMap::new();
    for s in &table.symbols {
        if s.live {
            decls.insert(
                s.id.clone(),
                Decl {
                    id: s.id.clone(),
                    scope: s.path.clone(),
                    name: s.name.clone(),
                    body: String::new(),
                    refs: BTreeSet::new(),
                },
            );
        }
    }
    (
        State {
            decls,
            files: BTreeMap::new(),
        },
        table,
    )
}

/// The typed ops for one side, computed against the shared base identity frame
/// so ids line up across sides.
pub fn aligned_ops(
    base_table: &SymbolTable,
    base_texts: &TextMap,
    side_texts: &TextMap,
) -> Vec<Op> {
    diff_to_ops(base_texts, side_texts, &mut base_table.clone())
}

/// The base file a typed op pertains to (its declaration's home file).
pub fn op_path(op: &Op, base_table: &SymbolTable) -> Option<String> {
    match op {
        Op::EditText { file, .. } => Some(file.clone()),
        Op::AddDecl { scope, .. } => Some(scope.clone()),
        _ => op
            .target_id()
            .and_then(|id| base_table.get(id).map(|s| s.path.clone())),
    }
}

/// Both sides of a three-way merge, aligned onto one base identity frame.
///
/// Construct once per merge and query per file: `renames_for` drives the
/// same-file rename/body-edit compose, `override_for` carries the cross-file
/// Move-vs-edit results that are resolved set-wise rather than per file.
pub struct Alignment {
    pub base_state: State,
    pub base_table: SymbolTable,
    pub ops_ours: Vec<Op>,
    pub ops_theirs: Vec<Op>,
    overrides: TextMap,
}

impl Alignment {
    /// Align both sides' edits onto the base identity frame.
    pub fn new(base_texts: &TextMap, ours_texts: &TextMap, theirs_texts: &TextMap) -> Self {
        let (base_state, base_table) = base_model(base_texts);
        let ops_ours = aligned_ops(&base_table, base_texts, ours_texts);
        let ops_theirs = aligned_ops(&base_table, base_texts, theirs_texts);
        let overrides = move_edit_overrides(
            &base_table,
            &base_state,
            base_texts,
            ours_texts,
            theirs_texts,
            &ops_ours,
            &ops_theirs,
        );
        Alignment {
            base_state,
            base_table,
            ops_ours,
            ops_theirs,
            overrides,
        }
    }

    /// The definitive merged text for a file settled by a cross-file
    /// Move-vs-body-edit compose, if any.
    pub fn override_for(&self, path: &str) -> Option<&String> {
        self.overrides.get(path)
    }

    /// The per-side rename maps for one file, gated on commutation.
    pub fn renames_for(&self, path: &str) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        idaware_renames(
            path,
            &self.base_table,
            &self.base_state,
            &self.ops_ours,
            &self.ops_theirs,
        )
    }
}

/// For one file: the per-side rename maps (base name -> new name), but only
/// when every op pair the two sides made on the *same* declaration commutes
/// (the Engine-B gate). Returns empty maps otherwise, which disables the
/// identity-aware compose and preserves the existing merge behaviour.
pub fn idaware_renames(
    path: &str,
    base_table: &SymbolTable,
    base_state: &State,
    ops_ours: &[Op],
    ops_theirs: &[Op],
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let empty = (BTreeMap::new(), BTreeMap::new());
    let here = |ops: &[Op]| -> Vec<Op> {
        ops.iter()
            .filter(|o| op_path(o, base_table).as_deref() == Some(path))
            .cloned()
            .collect()
    };
    let ours_here = here(ops_ours);
    let theirs_here = here(ops_theirs);
    for a in &ours_here {
        for b in &theirs_here {
            if a.target_id().is_some()
                && a.target_id() == b.target_id()
                && !commute(base_state, a, b, None, None)
            {
                return empty;
            }
        }
    }
    let renames = |ops: &[Op]| -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for op in ops {
            if let Op::Rename { id, new_name } = op {
                if let Some(sym) = base_table.get(id) {
                    m.insert(sym.name.clone(), new_name.clone());
                }
            }
        }
        m
    };
    (renames(&ours_here), renames(&theirs_here))
}

/// Cross-file Move-vs-body-edit compose (Engine B, multi-path): when one side
/// moved a declaration to another file and the other side edited its body in
/// place, the ops commute — produce the merged text for both files directly
/// (declaration lands at the new location with the edited body). Conservative:
/// only fires when each side touched nothing else in the involved files.
pub fn move_edit_overrides(
    base_table: &SymbolTable,
    base_state: &State,
    base_texts: &TextMap,
    ours_texts: &TextMap,
    theirs_texts: &TextMap,
    ops_ours: &[Op],
    ops_theirs: &[Op],
) -> TextMap {
    let mut overrides: TextMap = BTreeMap::new();
    let mut poisoned: BTreeSet<String> = BTreeSet::new();
    let sides = [
        (ops_ours, ours_texts, ops_theirs, theirs_texts),
        (ops_theirs, theirs_texts, ops_ours, ours_texts),
    ];
    for (mover_ops, mover_texts, editor_ops, editor_texts) in sides {
        for op in mover_ops {
            let Op::Move { id, new_scope } = op else {
                continue;
            };
            let mover_on_id = mover_ops
                .iter()
                .filter(|o| o.target_id() == Some(id))
                .count();
            let editor_on_id: Vec<&Op> = editor_ops
                .iter()
                .filter(|o| o.target_id() == Some(id))
                .collect();
            if mover_on_id != 1 || editor_on_id.len() != 1 {
                continue;
            }
            let edit = editor_on_id[0];
            if !matches!(edit, Op::ModifyBody { .. }) || !commute(base_state, op, edit, None, None)
            {
                continue;
            }
            let Some(sym) = base_table.get(id) else {
                continue;
            };
            let (src, dst, name) = (sym.path.clone(), new_scope.clone(), sym.name.clone());
            if src == dst {
                continue;
            }
            let (Some(lang), Some(lang_dst)) = (lang_for_path(&src), lang_for_path(&dst)) else {
                continue;
            };
            if lang != lang_dst {
                continue;
            }
            if editor_texts.get(&dst) != base_texts.get(&dst) {
                continue;
            }
            let (Some(base_src), Some(mover_src), Some(editor_src), Some(mover_dst)) = (
                base_texts.get(&src),
                mover_texts.get(&src),
                editor_texts.get(&src),
                mover_texts.get(&dst),
            ) else {
                continue;
            };
            let Some((base_chunk, base_rest)) = extract_named_chunk(lang, base_src, &name) else {
                continue;
            };
            // Mover: pure removal at source, body-preserving insert at destination.
            if rough_tokens(mover_src) != rough_tokens(&base_rest) {
                continue;
            }
            let Some((moved_chunk, _)) = extract_named_chunk(lang, mover_dst, &name) else {
                continue;
            };
            if rough_tokens(&moved_chunk) != rough_tokens(&base_chunk) {
                continue;
            }
            // Editor: changed only this declaration in the source file.
            let Some((edited_chunk, editor_rest)) = extract_named_chunk(lang, editor_src, &name)
            else {
                continue;
            };
            if rough_tokens(&editor_rest) != rough_tokens(&base_rest) {
                continue;
            }
            let Some(merged_dst) = replace_named_chunk(lang, mover_dst, &name, &edited_chunk)
            else {
                continue;
            };
            if !parses_clean(lang, &merged_dst) || !parses_clean(lang, mover_src) {
                continue;
            }
            for (path, text) in [(src, mover_src.clone()), (dst, merged_dst)] {
                if overrides.insert(path.clone(), text).is_some() {
                    poisoned.insert(path);
                }
            }
        }
    }
    for path in poisoned {
        overrides.remove(&path);
    }
    overrides
}

/// The result of a set-wise identity-aware merge.
#[derive(Clone, Debug, Default)]
pub struct SetMerge {
    /// Merged text per path. A conflicted path is absent here.
    pub merged: TextMap,
    /// Conflicted paths mapped to their marked-up text.
    pub conflicts: TextMap,
    /// Paths deleted on one side and untouched on the other.
    pub deleted: BTreeSet<String>,
    /// How many files each merge strategy settled, for telemetry.
    pub strategies: BTreeMap<String, u32>,
    /// Names that the merge left dangling — see `hidden::hidden_conflicts`.
    pub hidden: BTreeSet<String>,
    /// Duplicate definitions and stale call arities the merge introduced.
    pub checks: BTreeSet<String>,
    /// Merged paths a bundled grammar covered, so `hidden` and `checks` apply
    /// to them. Every other merged path was line-merged only and carries no
    /// structural guarantee — see [`SetMerge::unverified`].
    pub structural: BTreeSet<String>,
}

impl SetMerge {
    /// True when nothing needs a human: no conflicts, no hidden breakage, no
    /// failed check, and every merged file was structurally verified.
    ///
    /// A set containing even one file in an unsupported language is not clean,
    /// because no structural claim can be made about that file. Hosts that want
    /// a weaker policy should read `structural` / [`SetMerge::unverified`] and
    /// decide explicitly.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
            && self.hidden.is_empty()
            && self.checks.is_empty()
            && self.unverified().is_empty()
    }

    /// Merged paths that received no structural analysis, only a line merge.
    pub fn unverified(&self) -> BTreeSet<String> {
        self.merged
            .keys()
            .filter(|p| !self.structural.contains(*p))
            .cloned()
            .collect()
    }

    /// Whether structural checks ran for `path`.
    pub fn was_structurally_checked(&self, path: &str) -> bool {
        self.structural.contains(path)
    }
}

/// Three-way merge a whole file set with the identity-aware engine.
///
/// This is the library entry point for hosts with their own storage: pass the
/// base, ours and theirs texts and get merged text back, plus the hidden
/// conflicts and result checks computed across the merged set. Files changed
/// on only one side commute and are taken verbatim; files changed on both go
/// through the structural merge with rename composition.
pub fn merge_texts(base: &TextMap, ours: &TextMap, theirs: &TextMap) -> SetMerge {
    let alignment = Alignment::new(base, ours, theirs);
    let mut out = SetMerge::default();

    let mut paths: BTreeSet<&String> = BTreeSet::new();
    paths.extend(base.keys());
    paths.extend(ours.keys());
    paths.extend(theirs.keys());

    for path in paths {
        if let Some(text) = alignment.override_for(path) {
            *out.strategies
                .entry("move_edit_compose".into())
                .or_insert(0) += 1;
            out.merged.insert(path.clone(), text.clone());
            continue;
        }
        let b = base.get(path);
        let o = ours.get(path);
        let t = theirs.get(path);
        let ours_changed = o != b;
        let theirs_changed = t != b;
        if !theirs_changed || o == t {
            match o {
                Some(text) => {
                    out.merged.insert(path.clone(), text.clone());
                }
                None => {
                    out.deleted.insert(path.clone());
                }
            }
            continue;
        }
        if !ours_changed {
            *out.strategies.entry("layer1_commute".into()).or_insert(0) += 1;
            match t {
                Some(text) => {
                    out.merged.insert(path.clone(), text.clone());
                }
                None => {
                    out.deleted.insert(path.clone());
                }
            }
            continue;
        }
        let (Some(ours_text), Some(theirs_text)) = (o, t) else {
            out.conflicts
                .insert(path.clone(), format!("delete/modify conflict on {path}"));
            continue;
        };
        let base_text = b.cloned().unwrap_or_default();
        let (lr, rr) = alignment.renames_for(path);
        let outcome = match merge_file_idaware(
            lang_for_path(path),
            &base_text,
            ours_text,
            theirs_text,
            &lr,
            &rr,
        ) {
            Ok(outcome) => outcome,
            Err(_) => {
                out.conflicts
                    .insert(path.clone(), format!("merge failed on {path}"));
                continue;
            }
        };
        for (k, v) in outcome.strategies {
            *out.strategies.entry(k).or_insert(0) += v;
        }
        if outcome.status == MergeStatus::Clean {
            out.merged.insert(path.clone(), outcome.text);
        } else {
            out.conflicts.insert(path.clone(), outcome.text);
        }
    }

    if out.conflicts.is_empty() {
        let per_lang = |texts: &TextMap, lang: &str| -> Vec<String> {
            texts
                .iter()
                .filter(|(p, _)| lang_for_path(p) == Some(lang))
                .map(|(_, t)| t.clone())
                .collect()
        };
        for lang in LANGS {
            let base_texts = per_lang(base, lang);
            let merged_texts = per_lang(&out.merged, lang);
            let base_refs: Vec<&str> = base_texts.iter().map(String::as_str).collect();
            let merged_refs: Vec<&str> = merged_texts.iter().map(String::as_str).collect();
            out.hidden
                .extend(hidden_conflicts(lang, &base_refs, &merged_refs));
            out.checks.extend(
                new_duplicate_definitions(lang, &base_refs, &merged_refs)
                    .into_iter()
                    .map(|d| format!("duplicate-definition: {d}")),
            );
            out.checks.extend(
                new_arity_mismatches(lang, &base_refs, &merged_refs)
                    .into_iter()
                    .map(|a| format!("stale-call-arity: {a}")),
            );
        }
        out.structural = out
            .merged
            .keys()
            .filter(|p| lang_for_path(p).is_some_and(|l| LANGS.contains(&l)))
            .cloned()
            .collect();
    }

    out
}
