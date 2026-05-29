//! Build-profile-aware application paths.
//!
//! A `bun tauri dev` run and an installed production build share the same
//! Tauri bundle identifier, so by default `app_data_dir()` resolves to the
//! SAME folder for both — they then contend on one `rustic.db` (SQLite WAL),
//! one rolling log, and one `file-history/` store, which is exactly why
//! launching dev while production is running fails or corrupts state.
//!
//! In debug builds we redirect every app-data lookup to a sibling
//! `<identifier>-dev` directory so the two run fully isolated. Release builds
//! are unchanged.

use std::path::PathBuf;
use tauri::{Manager, Runtime};

/// Resolve the app-data directory, isolated per build profile (see module
/// docs). All backend code should call this instead of
/// `app.path().app_data_dir()` directly.
pub fn app_data_dir<R: Runtime, M: Manager<R>>(app: &M) -> tauri::Result<PathBuf> {
    let base = app.path().app_data_dir()?;
    Ok(profile_scoped(base))
}

fn profile_scoped(base: PathBuf) -> PathBuf {
    if cfg!(debug_assertions) {
        let name = base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "rustic".to_string());
        base.with_file_name(format!("{name}-dev"))
    } else {
        base
    }
}
