//! Operation counters for performance regression guards.
//!
//! Wall-clock timing is noisy and machine-dependent; counting the *work*
//! (parses, file materializations, whole-log scans) is stable and is what the
//! audit findings are actually about. `lattice-merge` guards its merge path
//! the same way via `parsers::parse_count`.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! counter {
    ($name:ident, $count_fn:ident, $bump_fn:ident, $doc:literal) => {
        static $name: AtomicU64 = AtomicU64::new(0);

        #[doc = $doc]
        pub fn $count_fn() -> u64 {
            $name.load(Ordering::Relaxed)
        }

        /// Increment the counter. Public because `lattice-core` records the
        /// store-side events into the same registry.
        pub fn $bump_fn() {
            $name.fetch_add(1, Ordering::Relaxed);
        }
    };
}

counter!(DECL_PARSES, decl_parses, bump_decl_parse, "Tree-sitter parses run by `symbols::extract_decls`.");
counter!(FILE_MATERIALIZES, file_materializes, bump_file_materialize, "Files rebuilt from CST nodes by `snapshot::materialize_file`.");
counter!(EVENT_LOG_SCANS, event_log_scans, bump_event_log_scan, "Full reads of `events.jsonl`.");
counter!(SYMBOL_TABLE_LOADS, symbol_table_loads, bump_symbol_table_load, "Reads + parses of `symbols.json`.");
counter!(TREE_FLATTENS, tree_flattens, bump_tree_flatten, "Whole-tree walks by `snapshot::flatten_tree`.");

/// A snapshot of every counter, for before/after comparison in tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counters {
    pub decl_parses: u64,
    pub file_materializes: u64,
    pub event_log_scans: u64,
    pub symbol_table_loads: u64,
    pub tree_flattens: u64,
}

/// Read every counter at once.
pub fn snapshot() -> Counters {
    Counters {
        decl_parses: decl_parses(),
        file_materializes: file_materializes(),
        event_log_scans: event_log_scans(),
        symbol_table_loads: symbol_table_loads(),
        tree_flattens: tree_flattens(),
    }
}

impl Counters {
    /// Counters accumulated since an earlier snapshot.
    pub fn since(&self, earlier: &Counters) -> Counters {
        Counters {
            decl_parses: self.decl_parses - earlier.decl_parses,
            file_materializes: self.file_materializes - earlier.file_materializes,
            event_log_scans: self.event_log_scans - earlier.event_log_scans,
            symbol_table_loads: self.symbol_table_loads - earlier.symbol_table_loads,
            tree_flattens: self.tree_flattens - earlier.tree_flattens,
        }
    }
}

impl std::fmt::Display for Counters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "parses={} materializes={} log_scans={} symbol_loads={} flattens={}",
            self.decl_parses,
            self.file_materializes,
            self.event_log_scans,
            self.symbol_table_loads,
            self.tree_flattens
        )
    }
}
