import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { ZoomIn, ZoomOut, Maximize2 } from 'lucide-react';

// Shared zoom plumbing for the rendered previews (html, svg, markdown).
// The image and pdf previews grew their own copies of this first; these
// helpers are the extracted version so the remaining previews behave
// identically rather than each inventing its own zoom semantics.

/**
 * Zoom out / percentage / zoom in / fit controls for a preview toolbar.
 */
export function ZoomControls({
  scale,
  fitScale = 1,
  onScaleChange,
  minScale = 0.1,
  maxScale = 8,
  onFit,
  fitLabel = 'Fit to width',
}) {
  // The readout is relative to the fit baseline, so "100%" always means
  // "the whole thing fits the pane" — matching the image / pdf previews.
  const pct = Math.round((scale / (fitScale || 1)) * 100);
  return (
    <div className="flex items-center gap-1">
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={() => onScaleChange(Math.max(minScale, scale * 0.85))}
        aria-label="Zoom out"
        title="Zoom out"
      >
        <ZoomOut />
      </Button>
      <span className="w-12 text-center text-xs tabular-nums text-muted-foreground">
        {pct}%
      </span>
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={() => onScaleChange(Math.min(maxScale, scale * 1.15))}
        aria-label="Zoom in"
        title="Zoom in"
      >
        <ZoomIn />
      </Button>
      <Button size="icon-xs" variant="ghost" onClick={onFit} aria-label={fitLabel} title={fitLabel}>
        <Maximize2 />
      </Button>
    </div>
  );
}

/**
 * Tracks a scroll container's size and keeps a fit-to-pane scale in sync.
 *
 * `computeFit({ w, h })` returns the scale at which the content fits the
 * container. The returned `scale` snaps to that value on first measure and
 * follows it on every resize — but only while the user hasn't zoomed away
 * from it, so a manual zoom survives opening the explorer or chat dock.
 */
export function useFitZoom(scrollRef, computeFit, deps = []) {
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [scale, setScale] = useState(1);
  const fitRef = useRef(1);
  // Previous fit value, used to tell "user is sitting at fit" (follow the
  // new fit) from "user zoomed manually" (leave their scale alone).
  const lastFitRef = useRef(null);
  const computeRef = useRef(computeFit);
  computeRef.current = computeFit;

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    lastFitRef.current = null;
    const measure = () => {
      const node = scrollRef.current;
      if (!node) return;
      const w = node.clientWidth;
      const h = node.clientHeight;
      if (w <= 0 || h <= 0) return;
      setBox((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
      const fit = computeRef.current({ w, h });
      if (!Number.isFinite(fit) || fit <= 0) return;
      fitRef.current = fit;
      setScale((prev) => {
        const wasAtFit =
          lastFitRef.current != null && Math.abs(prev - lastFitRef.current) < 0.005;
        const next = lastFitRef.current == null || wasAtFit ? fit : prev;
        lastFitRef.current = fit;
        return next;
      });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  const fitNow = useCallback(() => {
    lastFitRef.current = fitRef.current;
    setScale(fitRef.current);
  }, []);

  return { box, scale, setScale, fitScale: fitRef.current, fitNow };
}

/**
 * Zooms on Ctrl/Cmd + wheel over `ref`, for previews that don't use
 * PreviewSurface (which carries its own copy of this listener).
 */
export function useCtrlWheelZoom(ref, { scale, onScaleChange, minScale = 0.25, maxScale = 4 }) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Non-passive so preventDefault sticks: otherwise WebView2 treats
    // Ctrl+wheel as a host page-zoom and shrinks the whole IDE.
    const onWheel = (e) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      e.stopPropagation();
      const current = scale > 0 ? scale : 1;
      const next = Math.min(maxScale, Math.max(minScale, current * Math.exp(-e.deltaY / 600)));
      if (next !== current) onScaleChange(next);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [ref, scale, onScaleChange, minScale, maxScale]);
}

/**
 * Spacer that keeps preview toolbars clear of the floating Edit ⇄ Preview
 * toggle that editor-pane.jsx overlays at the top-right of these panes.
 */
export function ToolbarToggleGap() {
  return <div className="w-24 shrink-0" aria-hidden />;
}
