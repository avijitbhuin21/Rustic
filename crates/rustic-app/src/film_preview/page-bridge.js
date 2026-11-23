// Injected first into every page of the live HTML preview. The editor shows a page as a
// Figma-style artboard that grows to the page's full height, so this script:
// - pins viewport-height units at the artboard's device height through `--rustic-vh`
//   (served CSS files are rewritten to use it; inline and script-made styles are fixed
//   here) and pins a viewport-sized <html>, so `100vh` doesn't grow with the frame;
// - reports the page's height;
// - hands wheel gestures, `#anchor` links and outside links to the editor.
// The editor is two frames up: page -> shell document -> editor. This is cooperation,
// not containment: the sandbox and CSP are the security boundary.
(() => {
  'use strict';
  // Same hurdle as the film bridge: a srcdoc child frame gets fresh constructors.
  for (const name of ['RTCPeerConnection', 'webkitRTCPeerConnection', 'mozRTCPeerConnection', 'RTCDataChannel', 'RTCIceTransport', 'RTCDtlsTransport', 'RTCSctpTransport']) {
    try { Object.defineProperty(window, name, { value: undefined, writable: false, configurable: false }); } catch (_) { /* not defined here */ }
  }
  const host = window.parent && window.parent.parent;
  if (!host || host === window) return;
  const post = (m) => {
    try { host.postMessage(Object.assign({ source: 'rustic-page' }, m), '*'); } catch (_) { /* editor gone */ }
  };
  const de = document.documentElement;
  const src = (document.currentScript && document.currentScript.src) || '';
  const base = src.indexOf('@rustic/') > 0 ? src.slice(0, src.indexOf('@rustic/')) : '';

  // A constructed sheet: page scripts rewriting the DOM can't drop it, and editing it raises no mutations.
  let sheet = null;
  let fallback = null;
  try {
    sheet = new CSSStyleSheet();
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  } catch (_) { sheet = null; }
  let V = 0;
  let pinHtml = false;
  const writeRules = () => {
    const css = `:root{--rustic-vh:${V / 100}px}${pinHtml ? `html{height:${V}px!important}` : ''}`;
    if (sheet) {
      try { sheet.replaceSync(css); return; } catch (_) { sheet = null; }
    }
    if (!fallback) fallback = document.createElement('style');
    fallback.textContent = css;
    if (!fallback.isConnected) (document.head || de).appendChild(fallback);
  };
  const setViewport = () => {
    if (V || window.innerHeight < 2) return;
    V = window.innerHeight;
    writeRules();
  };
  setViewport();

  const VH = 'var(--rustic-vh, 1vh)';
  const UNIT = /(^|[^\w.#-])([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)[dsl]?(vh|vmin|vmax)(?![\w-])/gi;
  const freeze = (v) => v.replace(UNIT, (m, pre, num, unit) => {
    const u = unit.toLowerCase();
    const inner = u === 'vh' ? VH : `${u === 'vmin' ? 'min' : 'max'}(1vw, ${VH})`;
    return `${pre}calc(${num} * ${inner})`;
  });
  const fixStyle = (st) => {
    for (let i = 0; i < st.length; i += 1) {
      const p = st[i];
      const v = st.getPropertyValue(p);
      if (!/v(h|min|max)/i.test(v) || v.indexOf('--rustic-vh') >= 0) continue;
      const next = freeze(v);
      if (next !== v) st.setProperty(p, next, st.getPropertyPriority(p));
    }
  };
  const walk = (rules) => {
    for (const r of rules) {
      if (r.style) fixStyle(r.style);
      if (r.cssRules) walk(r.cssRules);
    }
  };
  const fixSheet = (sh) => {
    if (sh === sheet) return;
    let rules = null;
    // Linked sheets are cross-origin to this opaque-origin page; the server already rewrote them.
    try { rules = sh.cssRules; } catch (_) { return; }
    if (rules) walk(rules);
  };
  const sweep = () => {
    for (const sh of document.styleSheets) fixSheet(sh);
    for (const sh of document.adoptedStyleSheets || []) fixSheet(sh);
    for (const el of document.querySelectorAll('[style*="vh" i],[style*="vmin" i],[style*="vmax" i]')) {
      if (el.style) fixStyle(el.style);
    }
  };
  // CSS-in-JS libraries add rules through insertRule, which raises no DOM mutation.
  const insertRule = CSSStyleSheet.prototype.insertRule;
  CSSStyleSheet.prototype.insertRule = function patchedInsertRule() {
    const at = insertRule.apply(this, arguments);
    try { if (this !== sheet && this.cssRules[at]) walk([this.cssRules[at]]); } catch (_) { /* unreadable */ }
    dirty = true;
    return at;
  };

  const scroller = () => document.scrollingElement || de;
  let floor = 0;
  let lastBox = -1;
  /** The page's height, independent of how tall the frame currently is where possible. */
  const measure = () => {
    const vp = window.innerHeight;
    const sh = scroller().scrollHeight;
    const rect = de.getBoundingClientRect();
    // An <html> sized to the viewport (height: 100%) would follow the frame as it grows.
    if (!pinHtml && V && vp === V && Math.abs(rect.height - vp) < 1 && sh > vp + 1) {
      pinHtml = true;
      writeRules();
      return measure();
    }
    const cs = getComputedStyle(de);
    const box = rect.height + (parseFloat(cs.marginTop) || 0) + (parseFloat(cs.marginBottom) || 0);
    if (Math.abs(box - lastBox) > 1) {
      floor = 0;
      lastBox = box;
    }
    // Overflow past <html> (positioned content) only shows while it overflows the frame; keep it.
    if (sh > vp + 1) floor = Math.max(floor, sh);
    return Math.ceil(Math.max(box, floor, V));
  };

  let sent = 0;
  let lastVp = 0;
  let loops = 0;
  let stuck = false;
  let dirty = true;
  const report = () => {
    timer = 0;
    setViewport();
    if (!V) return;
    if (dirty) {
      dirty = false;
      sweep();
    }
    const vp = window.innerHeight;
    const h = Math.min(measure(), 60000);
    if (h === sent) {
      lastVp = vp;
      return;
    }
    if (sent && vp !== lastVp) {
      // Grew by exactly as much as the frame did: something is sized to the frame (a script
      // reading innerHeight, say). Stop chasing it after a couple of rounds.
      const dv = vp - lastVp;
      const dh = h - sent;
      loops = dv > 0 && dh > 0 && Math.abs(dh - dv) <= 2 ? loops + 1 : 0;
      if (loops >= 2) stuck = true;
    }
    lastVp = vp;
    if (stuck && h > sent) return;
    sent = h;
    post({ type: 'size', height: h });
  };
  let timer = 0;
  const schedule = () => {
    if (!timer) timer = setTimeout(report, 50);
  };

  const changed = () => {
    dirty = true;
    schedule();
  };

  new MutationObserver(changed).observe(de, {
    childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['style', 'class'],
  });
  if (typeof ResizeObserver === 'function') {
    const ro = new ResizeObserver(schedule);
    ro.observe(de);
    document.addEventListener('DOMContentLoaded', () => { if (document.body) ro.observe(document.body); });
  }
  document.addEventListener('DOMContentLoaded', changed);
  window.addEventListener('load', changed);
  window.addEventListener('load', schedule, true);
  window.addEventListener('resize', schedule);
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(schedule, () => {});
  setInterval(schedule, 1000);
  schedule();

  const SCROLLS = /(auto|scroll|overlay)/;
  /** True when an element under the pointer can still scroll this way itself. */
  const innerScroll = (el, dx, dy) => {
    for (let n = el; n && n.nodeType === 1 && n !== document.body && n !== de; n = n.parentElement) {
      const cs = getComputedStyle(n);
      if (dy && SCROLLS.test(cs.overflowY) && n.scrollHeight > n.clientHeight + 1
        && (dy < 0 ? n.scrollTop > 0 : n.scrollTop + n.clientHeight < n.scrollHeight - 1)) return true;
      if (dx && SCROLLS.test(cs.overflowX) && n.scrollWidth > n.clientWidth + 1
        && (dx < 0 ? n.scrollLeft > 0 : n.scrollLeft + n.clientWidth < n.scrollWidth - 1)) return true;
    }
    return false;
  };
  window.addEventListener('wheel', (e) => {
    const zoom = e.ctrlKey || e.metaKey;
    if (e.defaultPrevented || (!zoom && innerScroll(e.target, e.deltaX, e.deltaY))) return;
    e.preventDefault();
    const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? (V || 800) : 1;
    post({
      type: 'wheel', dx: e.deltaX * k, dy: e.deltaY * k, ctrl: e.ctrlKey, meta: e.metaKey, shift: e.shiftKey, x: e.clientX, y: e.clientY,
    });
  }, { passive: false });

  window.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0) return;
    const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a) return;
    const href = a.getAttribute('href') || '';
    if (href.charAt(0) === '#') {
      let id = '';
      try { id = decodeURIComponent(href.slice(1)); } catch (_) { id = href.slice(1); }
      const el = id ? (document.getElementById(id) || document.getElementsByName(id)[0]) : de;
      if (!el) return;
      e.preventDefault();
      post({ type: 'anchor', y: el.getBoundingClientRect().top + window.scrollY });
      return;
    }
    const url = a.href;
    // Pages inside the project navigate in the frame; anything else is the editor's call.
    if (!url || (base && url.indexOf(base) === 0) || !/^(https?:|mailto:)/i.test(url)) return;
    e.preventDefault();
    post({ type: 'link', url: url.slice(0, 2000) });
  });
})();
