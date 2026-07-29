//! Typed operation set for the merge algebra (SPEC §2).

use super::model::{Decl, State, ROOT};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The typed operation set plus the content-anchored text fallback.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Add a new declaration with a fresh identity.
    AddDecl {
        id: String,
        scope: String,
        name: String,
        body: String,
        #[serde(default)]
        refs: BTreeSet<String>,
    },
    /// Delete an unreferenced, childless declaration.
    DeleteDecl { id: String },
    /// Replace a declaration's body and reference edges.
    ModifyBody {
        id: String,
        new_body: String,
        #[serde(default)]
        new_refs: BTreeSet<String>,
    },
    /// Rename a declaration; identity and reference edges untouched.
    Rename { id: String, new_name: String },
    /// Reparent a declaration into a new scope.
    Move { id: String, new_scope: String },
    /// Content-anchored fallback text edit: replace one unique line block.
    EditText {
        file: String,
        old_block: Vec<String>,
        new_block: Vec<String>,
        #[serde(default)]
        names_mentioned: BTreeSet<String>,
    },
}

impl Op {
    /// The declaration id a typed op targets, or None for text ops.
    pub fn target_id(&self) -> Option<&str> {
        match self {
            Op::AddDecl { id, .. }
            | Op::DeleteDecl { id }
            | Op::ModifyBody { id, .. }
            | Op::Rename { id, .. }
            | Op::Move { id, .. } => Some(id),
            Op::EditText { .. } => None,
        }
    }

    /// True for the text-fallback op.
    pub fn is_text(&self) -> bool {
        matches!(self, Op::EditText { .. })
    }
}

/// Count contiguous occurrences of `block` inside `lines`.
pub fn count_block(lines: &[String], block: &[String]) -> usize {
    if block.is_empty() || block.len() > lines.len() {
        return 0;
    }
    (0..=lines.len() - block.len())
        .filter(|&i| &lines[i..i + block.len()] == block)
        .count()
}

/// Return None if `op` applies to `state`, else the reason it cannot.
pub fn applicable(state: &State, op: &Op) -> Option<&'static str> {
    match op {
        Op::AddDecl {
            id,
            scope,
            name,
            refs,
            ..
        } => {
            if state.decls.contains_key(id) {
                Some("id exists")
            } else if !state.live(scope) {
                Some("scope dead")
            } else if state.name_taken(scope, name, None) {
                Some("name collision")
            } else if refs.iter().any(|r| !state.decls.contains_key(r)) {
                Some("dangling ref")
            } else {
                None
            }
        }
        Op::DeleteDecl { id } => {
            if !state.decls.contains_key(id) {
                Some("id dead")
            } else if !state.referenced_by(id).is_empty() {
                Some("still referenced")
            } else if !state.children(id).is_empty() {
                Some("has children")
            } else {
                None
            }
        }
        Op::ModifyBody { id, new_refs, .. } => {
            if !state.decls.contains_key(id) {
                Some("id dead")
            } else if new_refs.iter().any(|r| !state.decls.contains_key(r)) {
                Some("dangling ref")
            } else {
                None
            }
        }
        Op::Rename { id, new_name } => {
            if !state.decls.contains_key(id) {
                Some("id dead")
            } else if state.name_taken(&state.decls[id].scope, new_name, Some(id)) {
                Some("name collision")
            } else {
                None
            }
        }
        Op::Move { id, new_scope } => {
            if !state.decls.contains_key(id) {
                Some("id dead")
            } else if !state.live(new_scope) {
                Some("scope dead")
            } else if new_scope == id || state.descendants(id).contains(new_scope) {
                Some("scope cycle")
            } else if state.name_taken(new_scope, &state.decls[id].name, Some(id)) {
                Some("name collision")
            } else {
                None
            }
        }
        Op::EditText {
            file, old_block, ..
        } => {
            let Some(lines) = state.files.get(file) else {
                return Some("file missing");
            };
            if old_block.is_empty() {
                Some("empty anchor")
            } else if count_block(lines, old_block) != 1 {
                Some("anchor not unique")
            } else {
                None
            }
        }
    }
}

/// Apply `op` to `state`, returning Err(reason) if preconditions fail.
pub fn apply(state: &State, op: &Op) -> Result<State, &'static str> {
    if let Some(reason) = applicable(state, op) {
        return Err(reason);
    }
    let mut next = state.clone();
    match op {
        Op::AddDecl {
            id,
            scope,
            name,
            body,
            refs,
        } => {
            next.decls.insert(
                id.clone(),
                Decl {
                    id: id.clone(),
                    scope: scope.clone(),
                    name: name.clone(),
                    body: body.clone(),
                    refs: refs.clone(),
                },
            );
        }
        Op::DeleteDecl { id } => {
            next.decls.remove(id);
        }
        Op::ModifyBody {
            id,
            new_body,
            new_refs,
        } => {
            let d = next.decls.get_mut(id).unwrap();
            d.body = new_body.clone();
            d.refs = new_refs.clone();
        }
        Op::Rename { id, new_name } => {
            next.decls.get_mut(id).unwrap().name = new_name.clone();
        }
        Op::Move { id, new_scope } => {
            next.decls.get_mut(id).unwrap().scope = new_scope.clone();
        }
        Op::EditText {
            file,
            old_block,
            new_block,
            ..
        } => {
            let lines = next.files.get_mut(file).unwrap();
            let n = old_block.len();
            for i in 0..=lines.len() - n {
                if &lines[i..i + n] == old_block.as_slice() {
                    lines.splice(i..i + n, new_block.iter().cloned());
                    break;
                }
            }
        }
    }
    debug_assert!(next
        .decls
        .values()
        .all(|d| d.scope == ROOT || next.decls.contains_key(&d.scope)));
    Ok(next)
}

/// Apply a sequence of operations in order.
pub fn apply_all(state: &State, ops: &[Op]) -> Result<State, &'static str> {
    let mut cur = state.clone();
    for op in ops {
        cur = apply(&cur, op)?;
    }
    Ok(cur)
}
