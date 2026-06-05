// Web shim for `@tauri-apps/api/window`.
// The browser has no app-owned OS window to drive, so every method is a safe
// no-op. Returns a stable stub object so `getCurrentWindow().X()` never throws.

const noop = async () => {};

const stub = {
  minimize: noop,
  maximize: noop,
  unmaximize: noop,
  toggleMaximize: noop,
  close: noop,
  destroy: noop,
  setFocus: noop,
  show: noop,
  hide: noop,
  isMaximized: async () => false,
  isMinimized: async () => false,
  isFullscreen: async () => false,
  setFullscreen: noop,
  setTitle: noop,
  // Event subscriptions return an unlisten fn, matching the Tauri API.
  onResized: async () => () => {},
  onMoved: async () => () => {},
  onCloseRequested: async () => () => {},
  onFocusChanged: async () => () => {},
  listen: async () => () => {},
};

export function getCurrentWindow() {
  return stub;
}

// Older API name some code may use.
export function getCurrent() {
  return stub;
}

export class Window {
  constructor() {
    return stub;
  }
}
