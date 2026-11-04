// Two-column list/detail split with a draggable divider. Deliberately not
// react-resizable-panels: that library converts px limits through %, which
// misbehaves while a parent (e.g. a sliding Sheet) is still sizing, and its
// document-level capture listeners fight Radix dismiss logic in dialogs.
import React, { useCallback, useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

/** Read a stored width, falling back to `fallback`. */
function storedWidth(key, fallback) {
  if (!key) return fallback;
  const n = Number(localStorage.getItem(key));
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

/** Left pane of `width` px (persisted under `storageKey`) + flexible right pane. */
export function SplitPane({ left, right, storageKey, defaultWidth = 220, minWidth = 140, maxWidth = 520, minRight = 220, className }) {
  const rootRef = useRef(null);
  const [width, setWidth] = useState(() => storedWidth(storageKey && `rustic.split.${storageKey}`, defaultWidth));
  const [dragging, setDragging] = useState(false);

  const clamp = useCallback((w) => {
    const total = rootRef.current?.clientWidth || 0;
    const hardMax = total > 0 ? Math.max(minWidth, Math.min(maxWidth, total - minRight)) : maxWidth;
    return Math.round(Math.min(hardMax, Math.max(minWidth, w)));
  }, [minWidth, maxWidth, minRight]);

  // Keep the left pane inside its limits when the container itself resizes.
  useEffect(() => {
    const el = rootRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const ro = new ResizeObserver(() => setWidth((w) => clamp(w)));
    ro.observe(el);
    return () => ro.disconnect();
  }, [clamp]);

  const persist = (w) => { if (storageKey) localStorage.setItem(`rustic.split.${storageKey}`, String(w)); };

  const onPointerDown = (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragging(true);
  };
  const onPointerMove = (e) => {
    if (!dragging) return;
    const rect = rootRef.current?.getBoundingClientRect();
    if (rect) setWidth(clamp(e.clientX - rect.left));
  };
  const onPointerUp = (e) => {
    if (!dragging) return;
    e.stopPropagation();
    try { e.currentTarget.releasePointerCapture(e.pointerId); } catch { /* already released */ }
    setDragging(false);
    setWidth((w) => { persist(w); return w; });
  };
  const onKeyDown = (e) => {
    const step = e.shiftKey ? 40 : 12;
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    setWidth((w) => { const n = clamp(w + (e.key === 'ArrowRight' ? step : -step)); persist(n); return n; });
  };
  const reset = () => { const n = clamp(defaultWidth); setWidth(n); persist(n); };

  return (
    <div ref={rootRef} className={cn('flex min-h-0 min-w-0 flex-1', dragging && 'select-none', className)}>
      <div className="flex min-h-0 shrink-0 flex-col overflow-hidden" style={{ width }}>{left}</div>
      <div
        role="separator"
        aria-orientation="vertical"
        aria-valuenow={width}
        aria-valuemin={minWidth}
        aria-valuemax={maxWidth}
        tabIndex={0}
        title="Drag to resize · double-click to reset"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onDoubleClick={reset}
        onKeyDown={onKeyDown}
        className={cn(
          'relative w-px shrink-0 cursor-col-resize bg-border outline-none',
          'after:absolute after:inset-y-0 after:-left-1.5 after:w-3 after:content-[""]',
          'hover:bg-primary/60 focus-visible:bg-primary',
          dragging && 'bg-primary',
        )}
      />
      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">{right}</div>
    </div>
  );
}

export default SplitPane;
