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

        // Kill every live harness CLI child process before tearing the app
        // down. Without this, on Windows the Node-side `claude.cmd` shim can
        // outlive the Tauri host and orphan a tree of `node.exe` processes
        // until the user reboots. Block on the shutdown so the Job Object
        // handles get a chance to fire kernel-side termination.
        let registry = state.harness_registry.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(rt) = rt {
            rt.block_on(async {
                registry.shutdown_all().await;
            });
        }
    }
    app.exit(0);
}
