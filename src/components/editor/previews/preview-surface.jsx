import React, { useCallback, useEffect, useRef } from 'react';
import { cn } from '@/lib/utils';

// Shared scrollable container for media previews (image, pdf, video).
//
// Behavior:
//   - Paints a checkerboard background (via the .preview-checkerboard CSS
//     class in globals.css) so transparent images and pdf margins read
//     correctly on dark/light themes.
//   - Captures Ctrl/Cmd + wheel locally to drive the preview's own zoom
//     (`scale` + `onScaleChange`). Without this capture the WebView2 host
//     swallows Ctrl+wheel as a global page-zoom and the whole IDE shrinks.
//   - Children control the rendered content. Pass `toolbar` for the
//     header strip; everything else goes into the scroll area.
//
// `onScaleChange(next)` receives the next absolute scale (already
// clamped). Pass null/undefined to disable wheel zoom.
export function PreviewSurface({
  toolbar,
  children,
  scale,
  onScaleChange,
  minScale = 0.1,
  maxScale = 8,
  scrollRef,
  className,
}) {
  const innerRef = useRef(null);
  // Allow callers to read the scroll container via their own ref while we
  // still attach our own handlers — merge by writing through.
  const setRefs = useCallback(
    (el) => {
      innerRef.current = el;
      if (typeof scrollRef === 'function') scrollRef(el);
      else if (scrollRef) scrollRef.current = el;
    },
    [scrollRef],
  );

  // Capture-phase wheel listener so we can call preventDefault — React's
  // synthetic wheel handler defaults to passive, which means Ctrl+wheel
  // would still bubble up and trigger the host's page zoom. Adding the
  // listener directly with { passive: false } is the only reliable way
  // to suppress that.
  useEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const onWheel = (e) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (typeof onScaleChange !== 'function') return;
      e.preventDefault();
      e.stopPropagation();
      const current = typeof scale === 'number' && scale > 0 ? scale : 1;
      // Multiplicative zoom centered on cursor: small wheel deltas
      // shouldn't blow past 50% per tick. deltaY > 0 → zoom out.
      const factor = Math.exp(-e.deltaY / 600);
      const next = Math.min(maxScale, Math.max(minScale, current * factor));
      onScaleChange(next);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [scale, onScaleChange, minScale, maxScale]);

  return (
    <div className={cn('flex h-full w-full flex-col', className)}>
      {toolbar && (
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-border bg-muted/20 px-2">
          {toolbar}
        </div>
      )}
      <div
        ref={setRefs}
        className="preview-checkerboard relative flex-1 overflow-auto"
      >
        {children}
      </div>
    </div>
  );
}

export default PreviewSurface;
