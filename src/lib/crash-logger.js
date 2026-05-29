// Capture renderer-side errors and forward them to the Rust rolling log via
// the `log_frontend_error` command. Renderer crashes never reach the backend
// panic hook and vanish when the webview tears down, which is exactly the
// "app suddenly crashes with no log" situation we need visibility into.

import { invoke } from '@tauri-apps/api/core';

let installed = false;

function report(kind, message, source, stack) {
  try {
    // Always echo to the devtools console too — handy while developing.
    // eslint-disable-next-line no-console
    console.error(`[crash:${kind}]`, message, stack || '');
  } catch {}
  try {
    invoke('log_frontend_error', {
      kind,
      message: String(message ?? ''),
      source: source ?? null,
      stack: stack ?? null,
    }).catch(() => {});
  } catch {
    /* invoke unavailable (browser preview) — console.error above is the record */
  }
}

export function installGlobalErrorHandlers() {
  if (installed || typeof window === 'undefined') return;
  installed = true;

  window.addEventListener('error', (e) => {
    const msg = e?.error?.message || e?.message || 'Uncaught error';
    const stack = e?.error?.stack || '';
    const source = e?.filename ? `${e.filename}:${e.lineno}:${e.colno}` : '';
    report('error', msg, source, stack);
  });

  window.addEventListener('unhandledrejection', (e) => {
    const reason = e?.reason;
    const msg = reason?.message || String(reason ?? 'Unhandled promise rejection');
    const stack = reason?.stack || '';
    report('unhandledrejection', msg, '', stack);
  });
}

export function logReactError(error, info) {
  report(
    'react-error-boundary',
    error?.message || String(error),
    '',
    `${error?.stack || ''}\n${info?.componentStack || ''}`,
  );
}
