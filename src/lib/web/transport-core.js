// Web transport core: turns the desktop `invoke()` / `listen()` calls into
// HTTP POSTs and a single multiplexed WebSocket against `rustic-server`.
//
// This module is only bundled in the `VITE_TARGET=web` build (wired via Vite
// aliases — see vite.config.js). The desktop build never imports it and keeps
// talking to Tauri directly.

const TOKEN_KEY = 'rustic_session_token';

function getToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || '';
  } catch {
    return '';
  }
}

function setToken(t) {
  try {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* ignore storage errors (private mode) */
  }
}

// ---- login gate -----------------------------------------------------------
//
// When the API returns 401 we render a minimal password overlay and resolve
// once the user logs in successfully. A single in-flight login promise is
// shared so concurrent 401s queue behind one prompt rather than stacking
// overlays.

let loginPromise = null;

function showLogin() {
  if (loginPromise) return loginPromise;

  loginPromise = new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.style.cssText =
      'position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;' +
      'justify-content:center;background:#0b0d10;font-family:system-ui,sans-serif';
    overlay.innerHTML = `
      <form id="rustic-login" style="background:#16191d;padding:28px;border-radius:12px;
        box-shadow:0 8px 40px rgba(0,0,0,.5);width:300px;display:flex;flex-direction:column;gap:12px">
        <div style="color:#e6e6e6;font-size:16px;font-weight:600">Rustic</div>
        <div style="color:#9aa0a6;font-size:13px">Enter the access password</div>
        <input id="rustic-pw" type="password" autocomplete="current-password" autofocus
          style="padding:10px;border-radius:8px;border:1px solid #2a2f36;background:#0e1013;color:#e6e6e6" />
        <div id="rustic-err" style="color:#ef5350;font-size:12px;min-height:16px"></div>
        <button type="submit"
          style="padding:10px;border-radius:8px;border:0;background:#3b82f6;color:#fff;font-weight:600;cursor:pointer">
          Unlock
        </button>
      </form>`;
    document.body.appendChild(overlay);

    const form = overlay.querySelector('#rustic-login');
    const pw = overlay.querySelector('#rustic-pw');
    const err = overlay.querySelector('#rustic-err');

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      err.textContent = '';
      try {
        const res = await fetch('/login', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify({ password: pw.value }),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
          err.textContent = data.error || 'Login failed';
          return;
        }
        setToken(data.token || '');
        overlay.remove();
        loginPromise = null;
        reconnectWs();
        resolve();
      } catch (ex) {
        err.textContent = String(ex);
      }
    });
  });

  return loginPromise;
}

// ---- invoke (HTTP) ---------------------------------------------------------

export async function invoke(command, args = {}) {
  const doFetch = () =>
    fetch(`/api/${command}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(getToken() ? { Authorization: `Bearer ${getToken()}` } : {}),
      },
      credentials: 'same-origin',
      body: JSON.stringify(args ?? {}),
    });

  let res = await doFetch();

  if (res.status === 401) {
    await showLogin();
    res = await doFetch(); // retry once after login
  }

  // Tauri's invoke resolves with the value or rejects with the error string.
  // Mirror that exactly so existing `.catch()` sites behave the same.
  const text = await res.text();
  let data;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = text;
  }

  if (!res.ok) {
    const message =
      data && typeof data === 'object' && 'error' in data ? data.error : data;
    throw new Error(typeof message === 'string' ? message : `HTTP ${res.status}`);
  }
  return data;
}

// Tauri exposes `convertFileSrc` to turn a filesystem path into a URL the
// webview can load. On the server we route binary reads through an endpoint.
export function convertFileSrc(filePath, _protocol = 'asset') {
  return `/api/asset?path=${encodeURIComponent(filePath)}`;
}

// ---- listen (WebSocket) ----------------------------------------------------

const listeners = new Map(); // event name -> Set<callback>
let ws = null;
let wsReconnectDelay = 500;
let wsConnecting = false;

function wsUrl() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = getToken();
  return `${proto}//${location.host}/ws${token ? `?token=${encodeURIComponent(token)}` : ''}`;
}

function connectWs() {
  if (wsConnecting || (ws && ws.readyState === WebSocket.OPEN)) return;
  wsConnecting = true;

  try {
    ws = new WebSocket(wsUrl());
  } catch {
    wsConnecting = false;
    scheduleReconnect();
    return;
  }

  ws.onopen = () => {
    wsConnecting = false;
    wsReconnectDelay = 500; // reset backoff on success
  };

  ws.onmessage = (e) => {
    let msg;
    try {
      msg = JSON.parse(e.data);
    } catch {
      return;
    }
    const set = listeners.get(msg.event);
    if (!set) return;
    // Match Tauri's event shape: { event, payload, id }.
    const evt = { event: msg.event, payload: msg.payload, id: 0 };
    for (const cb of set) {
      try {
        cb(evt);
      } catch (ex) {
        console.error('[transport] listener threw', ex);
      }
    }
  };

  ws.onclose = () => {
    wsConnecting = false;
    ws = null;
    scheduleReconnect();
  };

  ws.onerror = () => {
    try {
      ws && ws.close();
    } catch {
      /* ignore */
    }
  };
}

function scheduleReconnect() {
  if (listeners.size === 0) return; // nobody cares yet
  setTimeout(connectWs, wsReconnectDelay);
  wsReconnectDelay = Math.min(wsReconnectDelay * 2, 10000); // cap at 10s
}

function reconnectWs() {
  try {
    ws && ws.close();
  } catch {
    /* ignore */
  }
  ws = null;
  wsReconnectDelay = 500;
  connectWs();
}

// Push a terminal keystroke up the already-open WebSocket instead of issuing a
// fresh HTTP POST per character (which adds a full request round-trip of latency
// on remote deploys). Returns true if the frame was handed to an OPEN socket;
// false otherwise so the caller can fall back to the HTTP `write_terminal`.
export function sendTerminalInput(sessionId, data) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    try {
      ws.send(JSON.stringify({ t: 'terminal-input', sessionId, data }));
      return true;
    } catch {
      return false;
    }
  }
  return false;
}

// Tauri's `listen` returns a Promise<UnlistenFn>.
export async function listen(event, handler) {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(handler);
  connectWs();

  return () => {
    const s = listeners.get(event);
    if (s) {
      s.delete(handler);
      if (s.size === 0) listeners.delete(event);
    }
  };
}

// `once`: listen, then auto-unlisten after the first event.
export async function once(event, handler) {
  const unlisten = await listen(event, (evt) => {
    unlisten();
    handler(evt);
  });
  return unlisten;
}

// `emit` from the client is a no-op on the server transport — the desktop used
// it for window-local events the browser build doesn't need. Kept for API
// compatibility so imports don't break.
export async function emit(_event, _payload) {
  /* no-op in the web build */
}

export const TauriEvent = {};

// ---- download (HTTP GET → blob) -------------------------------------------

/// Download a server path as a browser file save. Folders arrive as a generated
/// zip (the server sets the filename via Content-Disposition); files arrive raw.
/// Authenticates with the session token and re-prompts on 401.
export async function downloadPath(path) {
  const doFetch = () =>
    fetch(`/api/download?path=${encodeURIComponent(path)}`, {
      method: 'GET',
      headers: { ...(getToken() ? { Authorization: `Bearer ${getToken()}` } : {}) },
      credentials: 'same-origin',
    });

  let res = await doFetch();
  if (res.status === 401) {
    await showLogin();
    res = await doFetch();
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    let message = text;
    try {
      const j = JSON.parse(text);
      if (j && j.error) message = j.error;
    } catch {
      /* keep raw text */
    }
    throw new Error(message || `HTTP ${res.status}`);
  }

  const blob = await res.blob();
  const fname = filenameFromDisposition(res.headers.get('Content-Disposition')) || baseName(path);
  triggerBlobDownload(blob, fname);
}

function filenameFromDisposition(header) {
  if (!header) return null;
  const m = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(header);
  return m ? decodeURIComponent(m[1]) : null;
}

function baseName(p) {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || 'download';
}

function triggerBlobDownload(blob, filename) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ---- upload (File → chunked raw stream → server) ---------------------------

const UPLOAD_CHUNK_BYTES = 64 * 1024 * 1024;
const UPLOAD_CHUNK_RETRIES = 3;

/// Upload one browser `File` into `dstDir` by streaming it in 64MB raw-byte
/// chunks to `/api/upload_stream` — no base64, no size ceiling beyond disk.
/// `relativePath` (set for folder uploads) recreates the file's subtree under
/// `dstDir`. `onProgress(uploadedBytes, totalBytes)` fires after each chunk.
export async function uploadFile(dstDir, file, relativePath = null, onProgress = null) {
  const total = file.size;
  let path = null;
  let offset = 0;

  do {
    const end = Math.min(offset + UPLOAD_CHUNK_BYTES, total);
    const params = new URLSearchParams({ offset: String(offset) });
    if (path) {
      params.set('path', path);
    } else {
      params.set('dstDir', dstDir);
      params.set('name', file.name);
      if (relativePath) params.set('relativePath', relativePath);
    }
    const data = await uploadChunk(params, file.slice(offset, end));
    path = data.path;
    offset = end;
    if (onProgress) onProgress(offset, total);
  } while (offset < total);

  return { path };
}

/// POST one chunk to /api/upload_stream with 401 re-auth and transient-error
/// retries; resolves with the server's `{ path, size }` JSON.
async function uploadChunk(params, blob) {
  const doFetch = () =>
    fetch(`/api/upload_stream?${params.toString()}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/octet-stream',
        ...(getToken() ? { Authorization: `Bearer ${getToken()}` } : {}),
      },
      credentials: 'same-origin',
      body: blob,
    });

  let lastError = null;
  for (let attempt = 0; attempt <= UPLOAD_CHUNK_RETRIES; attempt += 1) {
    if (attempt > 0) {
      await new Promise((r) => setTimeout(r, 1000 * 2 ** (attempt - 1)));
    }
    let res;
    try {
      res = await doFetch();
    } catch (ex) {
      lastError = ex;
      continue;
    }
    if (res.status === 401) {
      await showLogin();
      try {
        res = await doFetch();
      } catch (ex) {
        lastError = ex;
        continue;
      }
    }
    if (res.status === 502 || res.status === 503 || res.status === 504) {
      lastError = new Error(`HTTP ${res.status}`);
      continue;
    }
    const text = await res.text();
    let data;
    try {
      data = text ? JSON.parse(text) : null;
    } catch {
      data = text;
    }
    if (!res.ok) {
      const message =
        data && typeof data === 'object' && 'error' in data ? data.error : data;
      throw new Error(typeof message === 'string' ? message : `HTTP ${res.status}`);
    }
    return data;
  }
  throw lastError || new Error('upload failed');
}
