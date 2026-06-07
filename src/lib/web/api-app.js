// Web shim for `@tauri-apps/api/app`.
// `__APP_VERSION__` is injected from package.json at bundle time (see
// vite.config.js `define`), so this stays in sync with every release
// automatically. The literal is only a last-resort fallback.
const VERSION =
  (typeof __APP_VERSION__ !== 'undefined' && __APP_VERSION__) ||
  import.meta.env?.VITE_APP_VERSION ||
  '0.4.0';

export async function getVersion() {
  return VERSION;
}

export async function getName() {
  return 'Rustic';
}

export async function getTauriVersion() {
  return '0.0.0-web';
}
