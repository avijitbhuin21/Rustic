// Lightweight toast notification system.
//
// Usage:
//   import { showToast } from './components/toast.js';
//   showToast('Saved');                          // info, 3s
//   showToast('Save failed', { kind: 'error' }); // error, sticky until clicked
//   showToast('Building…', { kind: 'info', duration: 0 }); // sticky
//
// Tied into `window.unhandledrejection` so any unhandled promise rejection
// surfaces as a visible toast instead of silently logging to console.

import { el } from '../utils/dom.js';

let host = null;
let installed = false;

function ensureHost() {
  if (host && document.body.contains(host)) return host;
  host = el('div', {
    class: 'toast-host',
    role: 'status',
    'aria-live': 'polite',
    'aria-atomic': 'true',
  });
  document.body.appendChild(host);
  return host;
}

/**
 * Show a toast.
 * @param {string} message
 * @param {object} [opts]
 * @param {'info'|'success'|'warning'|'error'} [opts.kind='info']
 * @param {number} [opts.duration]  ms; 0 = sticky until clicked. Defaults: error/warning 6000, others 3000.
 * @param {string} [opts.action]    optional action label. If provided, opts.onAction is called when clicked.
 * @param {() => void} [opts.onAction]
 */
export function showToast(message, opts = {}) {
  const kind = opts.kind || 'info';
  const duration = opts.duration ?? (kind === 'error' || kind === 'warning' ? 6000 : 3000);
  const root = ensureHost();

  const t = el('div', { class: `toast toast--${kind}`, role: 'alert' });
  const body = el('div', { class: 'toast__body' }, message);
  t.appendChild(body);

  if (opts.action && typeof opts.onAction === 'function') {
    const btn = el('button', { class: 'toast__action' }, opts.action);
    btn.addEventListener('click', () => {
      try { opts.onAction(); } finally { dismiss(); }
    });
    t.appendChild(btn);
  }

  const closeBtn = el('button', {
    class: 'toast__close',
    'aria-label': 'Dismiss notification',
  }, '×');
  closeBtn.addEventListener('click', dismiss);
  t.appendChild(closeBtn);

  root.appendChild(t);

  let timer = null;
  if (duration > 0) {
    timer = setTimeout(dismiss, duration);
  }

  function dismiss() {
    if (timer) clearTimeout(timer);
    timer = null;
    if (t.parentNode === root) root.removeChild(t);
  }

  return { dismiss };
}

/**
 * Show an error toast given a thrown value (Error, string, anything).
 */
export function showErrorToast(prefix, err) {
  const msg = err instanceof Error ? err.message : String(err);
  showToast(`${prefix}: ${msg}`, { kind: 'error' });
}

/**
 * Install a global handler for unhandledrejection so any swallowed promise
 * error becomes a visible toast instead of a silent console log. Also handles
 * synchronous window errors. Idempotent.
 */
export function installGlobalErrorToasts() {
  if (installed) return;
  installed = true;
  window.addEventListener('unhandledrejection', (e) => {
    const reason = e.reason;
    // Skip cancelations and abort signals — those aren't user-actionable.
    if (reason && (reason.name === 'AbortError' || reason === 'cancelled')) return;
    showErrorToast('Unexpected error', reason);
  });
  window.addEventListener('error', (e) => {
    if (!e || !e.message) return;
    showErrorToast('Runtime error', e.message);
  });
}
