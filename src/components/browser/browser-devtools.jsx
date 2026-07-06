import React, { useCallback, useEffect, useRef, useState } from 'react';
import { devtoolsFrontendUrl } from '@/lib/browser-cdp';

// The embedded DevTools panel: the real Chrome DevTools frontend (Elements /
// Console / Network / Sources), served by Chromium through our authed proxy and
// pointed at the same page target's CDP socket. Same-origin asset requests
// authenticate via the session cookie; the CDP WebSocket carries a one-time
// auth ticket (with the cookie as the steady-state credential).

// DevTools surfaces its OWN device-mode preview (a second screencast of the
// page in a phone frame) whenever the page is under device-metrics emulation.
// Rustic's main viewport already renders + emulates the page, so that second
// screen is redundant — suppress it (and its toggle) so the panel is just the
// inspector. The DevTools structural containers are light DOM, but a few sit in
// shadow roots, so we inject the style into the document and any shadow roots.
const HIDE_CSS = `
  .device-mode-toolbar,
  .device-mode-content,
  .device-mode-view,
  .screencast,
  .screencast-view,
  [aria-label="Toggle device toolbar"],
  button[aria-label*="device toolbar" i] { display: none !important; }
`;

function injectInto(root, seen) {
  if (!root || seen.has(root)) return;
  seen.add(root);
  const doc = root.ownerDocument || root;
  const host = root.head || root;
  if (host && host.querySelector && !host.querySelector('style[data-rustic-dt]')) {
    const style = doc.createElement('style');
    style.setAttribute('data-rustic-dt', '');
    style.textContent = HIDE_CSS;
    host.appendChild(style);
  }
  // Descend into shadow roots (DevTools nests widgets in them).
  const els = root.querySelectorAll ? root.querySelectorAll('*') : [];
  for (const el of els) {
    if (el.shadowRoot) injectInto(el.shadowRoot, seen);
  }
}

export function BrowserDevtools({ targetId }) {
  // Re-key the iframe per target so switching tabs reloads the inspector
  // against the new page rather than a stale connection. The URL is built
  // asynchronously (it embeds a freshly minted one-time auth ticket).
  const [src, setSrc] = useState(null);
  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    if (!targetId) return undefined;
    devtoolsFrontendUrl(targetId)
      .then((url) => {
        if (!cancelled) setSrc(url);
      })
      .catch((e) => console.error('[devtools] failed to build frontend URL', e));
    return () => {
      cancelled = true;
    };
  }, [targetId]);
  const timers = useRef([]);

  const onLoad = useCallback((e) => {
    const iframe = e.currentTarget;
    // DevTools builds its UI asynchronously after load, and device mode can
    // engage later, so (re)inject a few times. CSS in <head> applies to nodes
    // created afterwards, so this only needs to cover shadow-root creation.
    timers.current.forEach(clearTimeout);
    timers.current = [0, 400, 1200, 2500].map((delay) =>
      setTimeout(() => {
        try {
          const doc = iframe.contentDocument;
          if (doc) injectInto(doc, new WeakSet());
        } catch {
          /* not ready / unexpected cross-origin — ignore */
        }
      }, delay),
    );
  }, []);

  if (!src) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#1e1e1e] text-xs text-muted-foreground">
        No active tab
      </div>
    );
  }

  return (
    <iframe
      key={targetId}
      src={src}
      title="DevTools"
      onLoad={onLoad}
      className="h-full w-full border-0 bg-[#1e1e1e]"
      // The inspector needs to run its own scripts + open its CDP WebSocket.
      sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
    />
  );
}
