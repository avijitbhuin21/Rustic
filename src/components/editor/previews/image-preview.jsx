import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { ZoomIn, ZoomOut, Maximize2 } from 'lucide-react';
import { basename } from '@/state/editor';
import { PreviewSurface } from './preview-surface';

const MIME = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  avif: 'image/avif',
};

function mimeFor(path) {
  const dot = path.lastIndexOf('.');
  const ext = dot < 0 ? '' : path.slice(dot + 1).toLowerCase();
  return MIME[ext] ?? 'application/octet-stream';
}

export default function ImagePreview({ tab }) {
  const [src, setSrc] = useState(null);
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);
  const [naturalDims, setNaturalDims] = useState(null);
  const [scale, setScale] = useState(1);
  // Baseline "fit-to-container" scale stored separately from the active
  // scale so the user's manual zoom doesn't get reset when the pane resizes.
  // The toolbar's % readout is computed relative to this baseline.
  const fitScaleRef = useRef(1);
  // Remember the previous fit value so the resize observer can tell whether
  // the user is currently at fit (snap to new fit) or zoomed away from it
  // (preserve their zoom). Without this any pane resize snapped manual zooms
  // back to fit, which felt like the zoom was capped to fit.
  const lastFitRef = useRef(null);
  const surfaceRef = useRef(null);
  // Pixel art (tiny icons, sprites) needs nearest-neighbour scaling at high
  // zoom; large photos look terrible nearest-neighboured. Pick smoothing
  // based on whichever the natural dims suggest is more likely.
  const wantsPixelated = naturalDims && (naturalDims.w <= 256 || naturalDims.h <= 256);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setSrc(null);
    setSize(null);
    setNaturalDims(null);
    invoke('read_file_base64', { path: tab.path })
      .then((res) => {
        if (cancelled) return;
        setSrc(`data:${mimeFor(tab.path)};base64,${res.data}`);
        setSize(res.size);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  // Recompute fit-to-container scale once dims + container are known. The
  // baseline lets the user "100%" mean "filled the pane" rather than the
  // image's raw native size, which is usually too big for the viewport.
  useLayoutEffect(() => {
    if (!naturalDims || !surfaceRef.current) return;
    lastFitRef.current = null;
    const compute = () => {
      const surface = surfaceRef.current;
      if (!surface) return;
      const availW = surface.clientWidth - 32;
      const availH = surface.clientHeight - 32;
      if (availW <= 0 || availH <= 0) return;
      const fit = Math.min(availW / naturalDims.w, availH / naturalDims.h, 1);
      const clampedFit = Math.max(0.05, fit);
      fitScaleRef.current = clampedFit;
      setScale((prev) => {
        // First compute for this image: snap to fit. Subsequent resizes:
        // only follow the new fit if the user was already at the previous
        // fit. If they had zoomed manually, leave their scale alone.
        const wasAtFit =
          lastFitRef.current != null &&
          Math.abs(prev - lastFitRef.current) < 0.01;
        const next =
          lastFitRef.current == null || wasAtFit ? clampedFit : prev;
        lastFitRef.current = clampedFit;
        return next;
      });
    };
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(surfaceRef.current);
    return () => ro.disconnect();
    // Don't depend on scale — that would snap user zooms back to fit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [naturalDims]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (!src) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-64 w-64" />
      </div>
    );
  }

  const displayW = naturalDims ? Math.max(1, Math.floor(naturalDims.w * scale)) : 0;
  const displayH = naturalDims ? Math.max(1, Math.floor(naturalDims.h * scale)) : 0;

  const toolbar = (
    <>
      <div className="truncate text-xs text-muted-foreground">
        {basename(tab.path)}
        {naturalDims && (
          <span className="ml-2 text-muted-foreground/60">
            {naturalDims.w} × {naturalDims.h}
          </span>
        )}
      </div>
      <div className="flex items-center gap-1">
        <Button
          size="icon-xs"
          variant="ghost"
          onClick={() => setScale((s) => Math.max(0.05, s * 0.85))}
          aria-label="Zoom out"
        >
          <ZoomOut />
        </Button>
        <span className="w-12 text-center text-xs text-muted-foreground">
          {Math.round((scale / (fitScaleRef.current || 1)) * 100)}%
        </span>
        <Button
          size="icon-xs"
          variant="ghost"
          onClick={() => setScale((s) => Math.min(64, s * 1.15))}
          aria-label="Zoom in"
        >
          <ZoomIn />
        </Button>
        <Button
          size="icon-xs"
          variant="ghost"
          onClick={() => setScale(fitScaleRef.current)}
          aria-label="Fit"
        >
          <Maximize2 />
        </Button>
        {size != null && (
          <span className="ml-2 text-[11px] text-muted-foreground">
            {(size / 1024).toFixed(1)} KB
          </span>
        )}
      </div>
    </>
  );

  return (
    <PreviewSurface
      toolbar={toolbar}
      scale={scale}
      onScaleChange={setScale}
      minScale={0.05}
      maxScale={64}
      scrollRef={surfaceRef}
    >
      <div className="flex min-h-full min-w-full items-center justify-center p-4">
        <img
          src={src}
          alt={basename(tab.path)}
          // Render at the exact computed pixel box. Using width/height
          // attributes (rather than CSS transform: scale) lets the scroll
          // container reserve the right amount of space at every zoom level
          // — without this, zoomed-in images get clipped and you can't
          // scroll to their right/bottom edges.
          style={{
            width: displayW || undefined,
            height: displayH || undefined,
            imageRendering: wantsPixelated && scale >= 2 ? 'pixelated' : 'auto',
          }}
          onLoad={(e) => {
            const img = e.currentTarget;
            setNaturalDims({ w: img.naturalWidth, h: img.naturalHeight });
          }}
          draggable={false}
        />
      </div>
    </PreviewSurface>
  );
}
