import React, { useEffect } from 'react';
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
