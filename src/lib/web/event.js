// Alias target for `@tauri-apps/api/event` in the web build.
// Re-exports the WebSocket-backed listen/once/emit from the transport core.
export { listen, once, emit, TauriEvent } from './transport-core.js';
