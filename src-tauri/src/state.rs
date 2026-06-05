//! Global application state.
//!
//! The definitions moved to the transport-agnostic `rustic-app` crate so the
//! headless server can share them. This module re-exports them unchanged so
//! every existing `crate::state::AppState` reference keeps resolving.

pub use rustic_app::state::*;
