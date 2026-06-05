// URL builders for the embedded VM browser (web build only).
//
// The screencast viewport and the embedded DevTools frontend both talk to
// Chromium's CDP through the authed server proxy:
//   • CDP WebSocket  → /ws/browser/cdp?target=<id>
//   • DevTools assets → /api/browser/devtools/*  (served by Chromium via proxy)
//
// The session token is carried as a query param on the WebSocket (the only
// thing a browser WS can send besides the auto-sent cookie). Same-origin
// DevTools asset requests authenticate via the HttpOnly session cookie set at
// login, so they need no extra plumbing.

const TOKEN_KEY = 'rustic_session_token';

function getToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || '';
  } catch {
    return '';
  }
}

/** The proxied CDP WebSocket URL for a page target. */
export function cdpWsUrl(targetId) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = getToken();
  const q = `target=${encodeURIComponent(targetId)}${token ? `&token=${encodeURIComponent(token)}` : ''}`;
  return `${proto}//${location.host}/ws/browser/cdp?${q}`;
}

/**
 * The embedded DevTools frontend URL for a page target. The bundled inspector
 * (served by Chromium through our proxy) connects to the CDP socket given in
 * its `ws`/`wss` param — which we point at our authed proxy path.
 */
export function devtoolsFrontendUrl(targetId, panel = 'elements') {
  const token = getToken();
  // DevTools wants host+path+query WITHOUT a scheme in the ws/wss param.
  const wsParam = `${location.host}/ws/browser/cdp?target=${encodeURIComponent(targetId)}${token ? `&token=${encodeURIComponent(token)}` : ''}`;
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  return `/api/browser/devtools/inspector.html?${scheme}=${encodeURIComponent(wsParam)}&panel=${panel}`;
}

/**
 * Open a short-lived CDP socket to a page target, send one command, resolve its
 * `result`, and close. Used for one-off page controls (reload / history nav)
 * that don't belong on the screencast socket.
 */
export function pageCommand(targetId, method, params = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let ws;
    try {
      ws = new WebSocket(cdpWsUrl(targetId));
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
