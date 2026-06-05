// `__IS_WEB__` is injected by Vite at build time (see vite.config.js `define`).
// true  → browser/server target (served by rustic-server)
// false → Tauri desktop target
export const IS_WEB = __IS_WEB__;
