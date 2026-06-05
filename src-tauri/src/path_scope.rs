//! Read/write path guards (system roots + sensitive home subdirs). Moved to
//! `rustic-app` so the server shares the exact same boundary; re-exported here
//! so `crate::path_scope::*` keeps resolving.

pub use rustic_app::path_scope::*;
