//! Commutation predicate implementing the SPEC §3.1 matrix, conservatively.
//! Per-op prefix contexts are mandatory (Phase 0 falsified base-state-only
//! commutation).

use super::model::State;
use super::ops::{apply, Op};
use std::collections::BTreeSet;
use std::mem::discriminant;

fn claim(state: &State, op: &Op) -> Option<(String, String)> {
    match op {
        Op::AddDecl { scope, name, .. } => Some((scope.clone(), name.clone())),
        Op::Rename { id, new_name } => state
            .decls
            .get(id)
            .map(|d| (d.scope.clone(), new_name.clone())),
        Op::Move { id, new_scope } => state
            .decls
            .get(id)
            .map(|d| (new_scope.clone(), d.name.clone())),
        _ => None,
    }
}

fn new_refs(op: &Op) -> BTreeSet<String> {
    match op {
        Op::AddDecl { refs, .. } => refs.clone(),
        Op::ModifyBody { new_refs, .. } => new_refs.clone(),
        _ => BTreeSet::new(),
    }
}

fn claimed_scope(op: &Op) -> Option<&str> {
    match op {
        Op::AddDecl { scope, .. } => Some(scope),
        Op::Move { new_scope, .. } => Some(new_scope),
        _ => None,
    }
}

fn typed_typed(sa: &State, sb: &State, a: &Op, b: &Op) -> bool {
    let (xa, xb) = (a.target_id(), b.target_id());
    if xa == xb {
        let kinds = (discriminant(a), discriminant(b));
        let rename = discriminant(&Op::Rename { id: String::new(), new_name: String::new() });
        let modify = discriminant(&Op::ModifyBody {
            id: String::new(),
            new_body: String::new(),
            new_refs: BTreeSet::new(),
        });
        let mv = discriminant(&Op::Move { id: String::new(), new_scope: String::new() });
        return kinds == (rename, modify)
            || kinds == (modify, rename)
            || kinds == (modify, mv)
            || kinds == (mv, modify);
    }
    let (ca, cb) = (claim(sa, a), claim(sb, b));
    if ca.is_some() && ca == cb {
        return false;
    }
    for ((p, sp, xp), (q, sq, xq)) in [((a, sa, xa), (b, sb, xb)), ((b, sb, xb), (a, sa, xa))] {
        if let Op::DeleteDecl { .. } = p {
            let xp = xp.unwrap();
            if new_refs(q).contains(xp) {
                return false;
            }
            if claimed_scope(q) == Some(xp) {
                return false;
            }
            if let Op::DeleteDecl { .. } = q {
                let xq = xq.unwrap();
                if sq.decls.get(xq).is_some_and(|d| d.refs.contains(xp)) {
                    return false;
                }
                if sp.decls.get(xp).is_some_and(|d| d.scope == xq) {
                    return false;
                }
            }
        }
        if let (Op::Move { new_scope, .. }, Op::Move { .. }) = (p, q) {
            let xq = xq.unwrap();
            if new_scope == xq || sq.descendants(xq).contains(new_scope) {
                return false;
            }
        }
    }
    true
}

fn text_typed(ctx_p: &State, t: &Op, p: &Op) -> bool {
    let Op::EditText { names_mentioned, .. } = t else {
        return true;
    };
    if matches!(p, Op::Rename { .. } | Op::DeleteDecl { .. }) {
        if let Some(d) = p.target_id().and_then(|id| ctx_p.decls.get(id)) {
            if names_mentioned.contains(&d.name) {
                return false;
            }
        }
    }
    if let Op::Rename { new_name, .. } = p {
        if names_mentioned.contains(new_name) {
            return false;
        }
    }
    true
}

fn text_text(state: &State, a: &Op, b: &Op) -> bool {
    let (Op::EditText { file: fa, .. }, Op::EditText { file: fb, .. }) = (a, b) else {
        return true;
    };
    if fa != fb {
        return true;
    }
    let ab = apply(state, a).and_then(|s| apply(&s, b));
    let ba = apply(state, b).and_then(|s| apply(&s, a));
    match (ab, ba) {
        (Ok(s1), Ok(s2)) => s1 == s2,
        _ => false,
    }
}

/// Return true iff `a` and `b` commute; contexts default to the shared base.
pub fn commute(base: &State, a: &Op, b: &Op, ctx_a: Option<&State>, ctx_b: Option<&State>) -> bool {
    if a == b {
        return true;
    }
    let sa = ctx_a.unwrap_or(base);
    let sb = ctx_b.unwrap_or(base);
    match (a.is_text(), b.is_text()) {
        (true, true) => text_text(base, a, b),
        (true, false) => text_typed(sb, a, b),
        (false, true) => text_typed(sa, b, a),
        (false, false) => typed_typed(sa, sb, a, b),
    }
}
