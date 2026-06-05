// Alias target for `@tauri-apps/api/core` in the web build.
// Re-exports the HTTP `invoke` + `convertFileSrc` from the transport core.
export { invoke, convertFileSrc } from './transport-core.js';

// `Channel` is used by some Tauri APIs for streaming; the web build streams via
// the WS hub instead, so provide a stub that throws if actually constructed.
export class Channel {
  constructor() {
    throw new Error('Tauri Channel is not available in the web build; use listen() over /ws');
  }
}
