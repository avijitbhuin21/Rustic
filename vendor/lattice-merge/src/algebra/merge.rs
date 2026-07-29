//! Merge of two changes from a common base: commuting ops compose, the rest
//! become divergences (SPEC §3.3).

use super::commute::commute;
use super::model::State;
use super::ops::{apply, apply_all, Op};
use std::collections::BTreeSet;

/// Outcome of merging two changes: a merged state, or the set of divergences.
#[derive(Clone, Debug)]
pub struct MergeResult {
    pub state: Option<State>,
    pub divergences: BTreeSet<BTreeSet<Op>>,
}

impl MergeResult {
    /// True when the merge produced a state with no divergences.
    pub fn clean(&self) -> bool {
        self.state.is_some() && self.divergences.is_empty()
    }
}

/// Merge two changes from `base`; non-commuting cross pairs diverge.
pub fn merge(base: &State, change_a: &[Op], change_b: &[Op]) -> Result<MergeResult, &'static str> {
    let mut ctx_a = Vec::new();
    let mut cur = base.clone();
    for op in change_a {
        ctx_a.push(cur.clone());
        cur = apply(&cur, op)?;
    }
    let mut ctx_b = Vec::new();
    cur = base.clone();
    for op in change_b {
        ctx_b.push(cur.clone());
        cur = apply(&cur, op)?;
    }
    let mut divergences: BTreeSet<BTreeSet<Op>> = BTreeSet::new();
    for (i, a) in change_a.iter().enumerate() {
        for (j, b) in change_b.iter().enumerate() {
            if a != b && !commute(base, a, b, Some(&ctx_a[i]), Some(&ctx_b[j])) {
                divergences.insert(BTreeSet::from([a.clone(), b.clone()]));
            }
        }
    }
    if !divergences.is_empty() {
        return Ok(MergeResult { state: None, divergences });
    }
    let b_rest: Vec<Op> = change_b
        .iter()
        .filter(|op| !change_a.contains(op))
        .cloned()
        .collect();
    let merged = apply_all(&apply_all(base, change_a)?, &b_rest)?;
    Ok(MergeResult { state: Some(merged), divergences })
}
