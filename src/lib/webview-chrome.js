// Keeps the webview's own browser chrome out of the desktop app: the native
// right-click menu (Reload / Inspect Element) and the inspector hotkey.

function isInspectorHotkey(event) {
  if (event.code !== 'KeyI') return false;
  return (event.metaKey && event.altKey) || (event.ctrlKey && event.shiftKey);
}

/** Suppress the webview's native context menu and inspector hotkey when running inside the desktop shell. */
export function installWebviewChromeGuard() {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;

  // Capture phase on window runs before Tauri's own document-level hotkey
  // listener, so the (capability-denied) toggle command is never invoked.
  window.addEventListener(
    'keydown',
    (event) => {
      if (!isInspectorHotkey(event)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
    },
    true,
  );

  // `vite dev` keeps the native menu so Inspect Element stays available while developing.
  if (import.meta.env.DEV) return;

  // Bubble phase on window runs after every component handler, so Rustic's
  // own context menus (which open from React handlers) are unaffected.
  window.addEventListener('contextmenu', (event) => event.preventDefault());
}
