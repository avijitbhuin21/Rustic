// Web shim for `@tauri-apps/api/app`.
// The build version is injected at bundle time via Vite's `define`/env; fall
// back to a constant matching the package version.
const VERSION = import.meta.env?.VITE_APP_VERSION || '0.3.9';

export async function getVersion() {
  return VERSION;
}

export async function getName() {
  return 'Rustic';
}

export async function getTauriVersion() {
  return '0.0.0-web';
}
