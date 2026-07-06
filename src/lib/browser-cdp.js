// URL builders for the embedded VM browser (web build only).
//
// The screencast viewport and the embedded DevTools frontend both talk to
// Chromium's CDP through the authed server proxy:
//   • CDP WebSocket  → /ws/browser/cdp?target=<id>
//   • DevTools assets → /api/browser/devtools/*  (served by Chromium via proxy)
//
// A browser WebSocket can't set an Authorization header, and the long-lived
// session token must never ride in a URL (proxies log query strings). Each WS
// URL instead carries a single-use short-TTL ticket minted by the authed
// `POST /api/ws_ticket` — plus the auto-sent HttpOnly session cookie as the
// steady-state credential. Same-origin DevTools asset requests authenticate
// via that cookie too, so they need no extra plumbing.

const TOKEN_KEY = 'rustic_session_token';

function getToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || '';
  } catch {
    return '';
  }
}

/**
 * Mint a one-time WS-auth ticket (see transport-core.js for the rationale;
 * duplicated here so this module stays free of web-build-only imports).
 * Returns '' on failure — the socket then relies on the session cookie alone.
 */
async function fetchTicket() {
  try {
    const token = getToken();
    const res = await fetch('/api/ws_ticket', {
      method: 'POST',
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      credentials: 'same-origin',
    });
    if (!res.ok) return '';
    const data = await res.json().catch(() => ({}));
    return typeof data.ticket === 'string' ? data.ticket : '';
  } catch {
    return '';
  }
}

/**
 * The proxied CDP WebSocket URL for a page target. Async: every call mints a
 * fresh single-use ticket, so callers must re-invoke it per (re)connect.
 */
export async function cdpWsUrl(targetId) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ticket = await fetchTicket();
  const q = `target=${encodeURIComponent(targetId)}${ticket ? `&ticket=${encodeURIComponent(ticket)}` : ''}`;
  return `${proto}//${location.host}/ws/browser/cdp?${q}`;
}

/**
 * The embedded DevTools frontend URL for a page target. The bundled inspector
 * (served by Chromium through our proxy) connects to the CDP socket given in
 * its `ws`/`wss` param — which we point at our authed proxy path. The embedded
 * ticket covers the inspector's initial connect; any later reconnect from the
 * (same-origin) DevTools iframe authenticates via the session cookie.
 */
export async function devtoolsFrontendUrl(targetId, panel = 'elements') {
  const ticket = await fetchTicket();
  // DevTools wants host+path+query WITHOUT a scheme in the ws/wss param.
  const wsParam = `${location.host}/ws/browser/cdp?target=${encodeURIComponent(targetId)}${ticket ? `&ticket=${encodeURIComponent(ticket)}` : ''}`;
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  return `/api/browser/devtools/inspector.html?${scheme}=${encodeURIComponent(wsParam)}&panel=${panel}`;
}

/**
 * Open a short-lived CDP socket to a page target, send one command, resolve its
 * `result`, and close. Used for one-off page controls (reload / history nav)
 * that don't belong on the screencast socket.
 */
export async function pageCommand(targetId, method, params = {}) {
  const url = await cdpWsUrl(targetId);
  return new Promise((resolve, reject) => {
    let settled = false;
    let ws;
    try {
      ws = new WebSocket(url);
    } catch (e) {
      reject(e);
      return;
    }
    const done = (fn, arg) => {
      if (settled) return;
      settled = true;
      try {
        ws.close();
      } catch {
        /* ignore */
      }
      fn(arg);
    };
    const timer = setTimeout(() => done(reject, new Error(`CDP ${method} timed out`)), 5000);
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method, params }));
    ws.onmessage = (e) => {
      let msg;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }
      if (msg.id === 1) {
        clearTimeout(timer);
        if (msg.error) done(reject, new Error(msg.error.message || 'CDP error'));
        else done(resolve, msg.result);
      }
    };
    ws.onerror = () => {
      clearTimeout(timer);
      done(reject, new Error(`CDP ${method} socket error`));
    };
  });
}

/** Reload the active page. */
export function pageReload(targetId) {
  return pageCommand(targetId, 'Page.reload', {});
}

/** Step the page's navigation history by `delta` (-1 back, +1 forward). */
export async function pageHistoryGo(targetId, delta) {
  const hist = await pageCommand(targetId, 'Page.getNavigationHistory', {});
  const idx = hist.currentIndex + delta;
  const entry = hist.entries?.[idx];
  if (!entry) return; // nothing to go to
  await pageCommand(targetId, 'Page.navigateToHistoryEntry', { entryId: entry.id });
}
