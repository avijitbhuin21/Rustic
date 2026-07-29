//! State model for the merge algebra: declarations plus text files (SPEC §1).

use std::collections::{BTreeMap, BTreeSet};

pub const ROOT: &str = "ROOT";

/// A declaration with a stable identity that survives rename and move.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Decl {
    pub id: String,
    pub scope: String,
    pub name: String,
    pub body: String,
    pub refs: BTreeSet<String>,
}

/// Immutable project state: declarations by id plus line-based text files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub decls: BTreeMap<String, Decl>,
    pub files: BTreeMap<String, Vec<String>>,
}

impl State {
    /// True if the id is ROOT or a live declaration.
    pub fn live(&self, decl_id: &str) -> bool {
        decl_id == ROOT || self.decls.contains_key(decl_id)
    }

    /// Declarations whose scope is the given id.
    pub fn children(&self, scope: &str) -> Vec<&Decl> {
        self.decls.values().filter(|d| d.scope == scope).collect()
    }

    /// True if a declaration other than `excluding` uses `name` in `scope`.
    pub fn name_taken(&self, scope: &str, name: &str, excluding: Option<&str>) -> bool {
        self.decls
            .values()
            .any(|d| d.scope == scope && d.name == name && Some(d.id.as_str()) != excluding)
    }

    /// Ids of all transitive scope-descendants of the given id.
    pub fn descendants(&self, decl_id: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut frontier: BTreeSet<String> = BTreeSet::from([decl_id.to_string()]);
        while !frontier.is_empty() {
            let next: BTreeSet<String> = self
                .decls
                .values()
                .filter(|d| frontier.contains(&d.scope))
                .map(|d| d.id.clone())
                .filter(|id| !out.contains(id))
                .collect();
            out.extend(next.iter().cloned());
            frontier = next;
        }
        out
    }

    /// Ids of declarations whose refs include the given id.
    pub fn referenced_by(&self, decl_id: &str) -> BTreeSet<String> {
        self.decls
            .values()
            .filter(|d| d.refs.contains(decl_id))
            .map(|d| d.id.clone())
            .collect()
    }

    /// Check invariants W1 (unique names per scope), W2 (live edges),
    /// W3 (acyclic scopes).
    pub fn well_formed(&self) -> bool {
        let mut seen = BTreeSet::new();
        for d in self.decls.values() {
            if !seen.insert((d.scope.clone(), d.name.clone())) {
                return false;
            }
            if d.scope != ROOT && !self.decls.contains_key(&d.scope) {
                return false;
            }
            if d.refs.iter().any(|r| !self.decls.contains_key(r)) {
                return false;
            }
        }
        for d in self.decls.values() {
            let mut cur = d.scope.clone();
            let mut hops = 0usize;
            while cur != ROOT {
                if cur == d.id || hops > self.decls.len() {
                    return false;
                }
                cur = self.decls[&cur].scope.clone();
                hops += 1;
            }
        }
        true
    }
}
