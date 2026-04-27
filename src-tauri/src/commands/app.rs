use tauri::{AppHandle, Manager};

/// Quit the app without further prompting. The frontend calls this after
/// it has confirmed that any dirty buffers are saved/discarded.
#[tauri::command]
pub fn confirm_quit(app: AppHandle) {
    // Best-effort WAL truncate so the -wal sidecar doesn't grow unbounded
    // across sessions. Failures here are non-fatal; the next launch will
    // simply pick up the existing -wal.
    if let Some(state) = app.try_state::<crate::state::AppState>() {
        if let Ok(db) = state.db.lock() {
            let _ = db.checkpoint_truncate();
        }
    }
    app.exit(0);
}
