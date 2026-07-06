/** Web-build shim for @tauri-apps/plugin-updater: the browser target has no self-update; check() always reports "no update". */
export async function check() {
  return null;
}
