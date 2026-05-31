import { useEffect } from 'react';

// Shared "copy code block" decorator for our markdown surfaces. All of them
// (agent chat, markdown file preview) render via dangerouslySetInnerHTML, so
// React never sees the <pre> nodes and we can't attach onClick the normal way.
// Instead we walk the freshly-injected DOM in a layout-safe effect, wrap each
// <pre> in a relatively-positioned container, and drop in an absolutely
// positioned copy button that surfaces on hover.
//
// Idempotency: every render replaces the container's innerHTML wholesale, so
// our wrappers/buttons vanish and the effect re-runs against fresh nodes. The
// `data-copy-decorated` flag guards against double-decorating within a single
// DOM generation (e.g. if the effect were ever invoked twice for the same html).

const COPY_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>';

const CHECK_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';

// Best-effort clipboard write. Prefers the async Clipboard API but falls back
// to a hidden textarea + execCommand for older WebView2 builds / non-secure
// contexts where navigator.clipboard is unavailable.
function copyText(text) {
  if (navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(text);
  }
  return new Promise((resolve) => {
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      ta.style.pointerEvents = 'none';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
    } catch {
      // best-effort — swallow
    }
    resolve();
  });
}

export function decorateCodeBlocks(container) {
  if (!container) return;
  const pres = container.querySelectorAll('pre');
  pres.forEach((pre) => {
    if (pre.dataset.copyDecorated) return;
    // Skip if the pre is already inside one of our wrappers (defensive).
    if (pre.parentElement?.classList.contains('rustic-code-wrap')) return;
    pre.dataset.copyDecorated = 'true';

    // Wrap the <pre> so the button can sit outside the horizontally
    // scrolling region — an absolute child of the pre itself would drift
    // when the code overflows and the user scrolls sideways.
    const wrap = document.createElement('div');
    wrap.className = 'rustic-code-wrap';
    pre.parentNode.insertBefore(wrap, pre);
    wrap.appendChild(pre);

    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'rustic-copy-btn';
    btn.setAttribute('aria-label', 'Copy code');
    btn.title = 'Copy code';
    btn.innerHTML = COPY_ICON;

    let resetTimer = null;
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      const code = pre.querySelector('code');
      // innerText preserves line breaks as the user sees them (textContent
      // collapses some whitespace nuances around block elements).
      const text = code ? code.innerText : pre.innerText;
      copyText(text).then(() => {
        btn.innerHTML = CHECK_ICON;
        btn.classList.add('copied');
        if (resetTimer) clearTimeout(resetTimer);
        resetTimer = setTimeout(() => {
          btn.innerHTML = COPY_ICON;
          btn.classList.remove('copied');
        }, 1200);
      });
    });

    wrap.appendChild(btn);
  });
}

// React hook: decorate the code blocks inside `ref.current` whenever the given
// dependencies change (typically the rendered HTML string). Pair it with the
// same ref used for dangerouslySetInnerHTML.
export function useCodeCopyButtons(ref, deps = []) {
  useEffect(() => {
    decorateCodeBlocks(ref.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
