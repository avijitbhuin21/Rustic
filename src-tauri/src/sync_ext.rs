//! Poison-resilient mutex locking. Moved to `rustic-app` so both transports
//! share it; re-exported here so `crate::sync_ext::MutexExt` keeps resolving.

pub use rustic_app::sync_ext::*;
