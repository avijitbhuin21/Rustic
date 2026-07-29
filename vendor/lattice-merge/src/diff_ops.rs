//! Structural diff of two in-memory snapshots into typed algebra ops.
//!
//! This is the front half of the identity-aware ("Engine B") merge: it turns
//! a pair of file maps into `Op`s keyed by stable symbol identity, reconciling
//! the `SymbolTable` as it goes so that identities survive rename and move.
//! Nothing here touches a repository or an on-disk format.

use crate::algebra::Op;
use crate::symbols::{extract_decls, RawDecl, SymbolEvent, SymbolTable};
use std::collections::{BTreeMap, BTreeSet};

/// True when `name` is a plain identifier, i.e. something that can appear as
/// a whole token in source text.
fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Split source text into identifier-shaped tokens.
fn identifier_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
}

/// Structurally diff two snapshots into typed ops, reconciling the symbol
/// table (identities survive rename/move). Files without decl-level events
/// fall back to content-anchored EditText ops.
pub fn diff_to_ops(
    old_texts: &BTreeMap<String, String>,
    new_texts: &BTreeMap<String, String>,
    table: &mut SymbolTable,
) -> Vec<Op> {
    let changed: BTreeSet<&String> = old_texts
        .keys()
        .chain(new_texts.keys())
        .filter(|p| old_texts.get(*p) != new_texts.get(*p))
        .collect();

    let mut extracted: BTreeMap<String, Vec<RawDecl>> = BTreeMap::new();
    // Only changed files are parsed. `reconcile` treats a path absent from
    // this map as "not looked at", so unchanged files keep their symbols
    // untouched instead of costing a tree-sitter parse each (PERF-3).
    for path in &changed {
        if let Some(text) = new_texts.get(*path) {
            let name = path.rsplit('/').next().unwrap_or(path);
            extracted.insert((*path).clone(), extract_decls(name, text));
        } else {
            extracted.insert((*path).clone(), Vec::new());
        }
    }

    let live_names: BTreeSet<String> = table
        .symbols
        .iter()
        .filter(|s| s.live)
        .map(|s| s.name.clone())
        .collect();

    let events = table.reconcile(&extracted);
    let mut ops = Vec::new();
    let mut files_with_events: BTreeSet<String> = BTreeSet::new();
    for event in events {
        match event {
            SymbolEvent::Added { id, path, name, .. } => {
                files_with_events.insert(path.clone());
                ops.push(Op::AddDecl {
                    id,
                    scope: path,
                    name,
                    body: String::new(),
                    refs: BTreeSet::new(),
                });
            }
            SymbolEvent::Deleted { id, path, .. } => {
                files_with_events.insert(path);
                ops.push(Op::DeleteDecl { id });
            }
            SymbolEvent::BodyModified { id, path, .. } => {
                files_with_events.insert(path.clone());
                let body = table
                    .get(&id)
                    .map(|s| s.body_hash.0.clone())
                    .unwrap_or_default();
                ops.push(Op::ModifyBody {
                    id,
                    new_body: body,
                    new_refs: BTreeSet::new(),
                });
            }
            SymbolEvent::Renamed {
                id, path, new_name, ..
            } => {
                files_with_events.insert(path);
                ops.push(Op::Rename { id, new_name });
            }
            SymbolEvent::Moved {
                id,
                old_path,
                new_path,
                ..
            } => {
                files_with_events.insert(old_path);
                files_with_events.insert(new_path.clone());
                ops.push(Op::Move {
                    id,
                    new_scope: new_path,
                });
            }
        }
    }

    for path in changed {
        if files_with_events.contains(path.as_str()) {
            continue;
        }
        let empty = String::new();
        let old = old_texts.get(path).unwrap_or(&empty);
        let new = new_texts.get(path).unwrap_or(&empty);
        let old_block: Vec<String> = old.lines().map(str::to_string).collect();
        let new_block: Vec<String> = new.lines().map(str::to_string).collect();
        // Each side is tokenized once and intersected, instead of scanning
        // the whole file body once per live symbol name (PERF-6). Names that
        // are not plain identifiers cannot appear as a token, so they keep
        // the substring test.
        let tokens: std::collections::HashSet<&str> = identifier_tokens(old)
            .chain(identifier_tokens(new))
            .collect();
        let mentioned: BTreeSet<String> = live_names
            .iter()
            .filter(|n| {
                if is_identifier(n) {
                    tokens.contains(n.as_str())
                } else {
                    old.contains(n.as_str()) || new.contains(n.as_str())
                }
            })
            .cloned()
            .collect();
        ops.push(Op::EditText {
            file: path.clone(),
            old_block,
            new_block,
            names_mentioned: mentioned,
        });
    }
    ops
}
