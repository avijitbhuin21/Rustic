// Bridges the web `@tauri-apps/plugin-dialog` shim to the in-app folder picker.
// `plugin-dialog.open()` calls `requestOpenDialog(options)`; the mounted
// <FolderPickerHost> registers a handler that renders the modal and resolves
// with the chosen path (or null on cancel). If no host is mounted the request
// resolves to null rather than hanging.

let handler = null;

/** Register the modal's open handler. Returns an unregister fn. */
export function setDialogHandler(fn) {
  handler = fn;
  return () => {
    if (handler === fn) handler = null;
  };
}

/** Ask the mounted picker to open; resolves with the selected path or null. */
export function requestOpenDialog(options = {}) {
  if (!handler) return Promise.resolve(null);
  return Promise.resolve(handler(options));
}
