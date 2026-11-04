// Finder-style selection for the sync trees: click selects one row,
// Cmd/Ctrl+click toggles, Shift+click selects the visible range from the
// last anchor. Also builds the multi-item drag preview badge.
import { useCallback, useRef, useState } from 'react';

/** Selection state over row keys; `entryOf(key)` maps a key to its payload. */
export function useTreeSelection() {
  const [sel, setSel] = useState(() => new Map());
  const anchor = useRef(null);

  /** Apply a row click; returns true when it was a plain (unmodified) click. */
  const click = useCallback((ev, key, value, visibleKeys, valueOf) => {
    const toggle = ev.metaKey || ev.ctrlKey;
    if (ev.shiftKey && anchor.current != null && visibleKeys?.length) {
      const a = visibleKeys.indexOf(anchor.current);
      const b = visibleKeys.indexOf(key);
      if (a !== -1 && b !== -1) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        setSel((cur) => {
          const next = toggle ? new Map(cur) : new Map();
          visibleKeys.slice(lo, hi + 1).forEach((k) => next.set(k, valueOf ? valueOf(k) : k));
          return next;
        });
        return false;
      }
    }
    anchor.current = key;
    if (toggle) {
      setSel((cur) => {
        const next = new Map(cur);
        if (next.has(key)) next.delete(key); else next.set(key, value);
        return next;
      });
      return false;
    }
    setSel(new Map([[key, value]]));
    return true;
  }, []);

  const clear = useCallback(() => { anchor.current = null; setSel(new Map()); }, []);

  return { sel, setSel, click, clear };
}

/** Use a small "N items" badge as the drag image when dragging several rows. */
export function setCountDragImage(dataTransfer, count, label) {
  if (count < 2 || typeof document === 'undefined') return;
  const el = document.createElement('div');
  el.textContent = `${count} items${label ? ` · ${label}` : ''}`;
  Object.assign(el.style, {
    position: 'fixed', top: '-1000px', left: '-1000px', padding: '4px 10px',
    borderRadius: '6px', font: '12px system-ui, sans-serif', whiteSpace: 'nowrap',
    background: 'var(--primary)', color: 'var(--primary-foreground)',
  });
  document.body.appendChild(el);
  try { dataTransfer.setDragImage(el, 12, 12); } catch { /* unsupported */ }
  setTimeout(() => el.remove(), 0);
}

/** Parent folder of a `/` or `\` separated path ('' for top-level relative paths). */
export function parentPath(p) {
  const s = String(p).replace(/[\\/]+$/, '');
  const i = Math.max(s.lastIndexOf('/'), s.lastIndexOf('\\'));
  return i === -1 ? '' : s.slice(0, i);
}
