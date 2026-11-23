// Injected first into every film page. Reports the page's contract (window.READY,
// window.DUR, window.EV) to the editor and draws a frame when asked.
// The editor is two frames up: page -> shell document -> editor.
(() => {
  'use strict';
  // CSP has no working control for WebRTC, which can send packets to any host.
  // Removing the constructors here only covers this page's own globals: a child
  // frame (srcdoc) gets fresh ones, so this is a hurdle, not a block. The film
  // view's per-project prompt is what the user relies on.
  for (const name of ['RTCPeerConnection', 'webkitRTCPeerConnection', 'mozRTCPeerConnection', 'RTCDataChannel', 'RTCIceTransport', 'RTCDtlsTransport', 'RTCSctpTransport']) {
    try { Object.defineProperty(window, name, { value: undefined, writable: false, configurable: false }); } catch (_) { /* not defined here */ }
  }
  const host = window.parent && window.parent.parent;
  if (!host || host === window) return;
  const text = (v) => String(v == null ? '' : v).slice(0, 600);
  const post = (m) => {
    try { host.postMessage(Object.assign({ source: 'rustic-film' }, m), '*'); } catch (_) { /* editor gone */ }
  };

  window.addEventListener('error', (e) => {
    const el = e.target && e.target !== window ? e.target : null;
    const url = el && (el.src || el.href);
    post({ type: 'error', message: text(e.message || (url ? `Failed to load ${url}` : 'Script error')) });
  }, true);
  window.addEventListener('unhandledrejection', (e) => {
    post({ type: 'error', message: text(e.reason && (e.reason.message || e.reason)) });
  });

  const events = () => {
    const out = [];
    if (!Array.isArray(window.EV)) return out;
    for (const e of window.EV) {
      if (out.length >= 2000) break;
      const t = Number(e && e.t);
      if (Number.isFinite(t)) out.push({ t, type: text(e && e.type).slice(0, 40) });
    }
    return out;
  };

  let ready = false;
  const timer = setInterval(() => {
    if (window.READY !== true || typeof window.render !== 'function') return;
    ready = true;
    clearInterval(timer);
    post({ type: 'ready', dur: Number(window.DUR), events: events() });
  }, 50);

  window.addEventListener('message', (e) => {
    if (e.source !== host) return;
    const m = e.data;
    if (!m || m.source !== 'rustic-film-host' || m.type !== 'seek') return;
    const t = Number(m.t);
    if (!ready || !Number.isFinite(t)) return;
    try { window.render(t); } catch (err) { post({ type: 'error', message: text((err && err.message) || err) }); }
    post({ type: 'frame', t });
  });
})();
