//! Notebook execution: thin Tauri wrapper over the shared kernel core in
//! `rustic_app::notebook_kernel`. Replies are forwarded to the frontend as
//! `notebook-kernel-output` events. The same core backs the rustic-server
//! host (`rustic-server/src/commands/notebook.rs`) — keep them in sync.

use rustic_app::notebook_kernel as core;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

fn emitter(app: AppHandle) -> core::KernelEmit {
    Arc::new(move |ev: core::KernelEvent| {
        let _ = app.emit("notebook-kernel-output", ev);
    })
}

/// Start (or restart) the Python kernel for a notebook.
#[tauri::command]
pub fn notebook_kernel_start(
    app: AppHandle,
    notebook_id: String,
    cwd: String,
) -> Result<String, String> {
    core::start(&notebook_id, &cwd, emitter(app))
}

/// Send a cell's code to the notebook's kernel. The reply arrives as a
/// `notebook-kernel-output` event with kind="reply" and payload.id = cell_id.
#[tauri::command]
pub fn notebook_kernel_exec(
    notebook_id: String,
    cell_id: String,
    code: String,
) -> Result<(), String> {
    core::exec(&notebook_id, &cell_id, &code)
}

/// Stop the notebook's kernel (used for restart and on tab close).
#[tauri::command]
pub fn notebook_kernel_stop(notebook_id: String) -> Result<(), String> {
    core::stop(&notebook_id);
    Ok(())
}
