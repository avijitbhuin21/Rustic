// `__IS_WEB__` is injected by Vite at build time (see vite.config.js `define`).
// true  → browser/server target (served by rustic-server)
// false → Tauri desktop target
export const IS_WEB = __IS_WEB__;

/// True when a backend transport is available: always in the web build (the
/// HTTP/WS shims are compiled in), and in the desktop build only when the
/// page actually runs inside the Tauri webview. Previously copy-pasted into
/// ~14 files — this is the single canonical definition.
export function isTauriAvailable() {
  return IS_WEB || (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window);
}

/// True on iPhone/iPod and on iPads — modern iPadOS reports itself as
/// "Macintosh" in the UA, so a Mac UA combined with multi-touch is an iPad.
export function isIOS() {
  if (typeof navigator === 'undefined') return false;
  const ua = navigator.userAgent || '';
  if (/iPad|iPhone|iPod/.test(ua)) return true;
  return /Macintosh/.test(ua) && (navigator.maxTouchPoints || 0) > 1;
}

/// True in any WebKit/Safari browser (including every iOS browser, which is
/// WebKit under the hood). Chrome/Edge UAs contain "Safari" too, so exclude
/// Chromium markers.
export function isSafari() {
  if (typeof navigator === 'undefined') return false;
  const ua = navigator.userAgent || '';
  return /Safari/.test(ua) && !/Chrome|Chromium|CriOS|Edg/.test(ua);
}
