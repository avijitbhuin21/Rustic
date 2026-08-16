import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { ZoomIn, ZoomOut, Maximize2, RotateCw, Monitor } from 'lucide-react';
import { cn } from '@/lib/utils';

export const CANVAS_MIN_SCALE = 0.05;
export const CANVAS_MAX_SCALE = 8;

const FRAME_GAP = 64;
const FIT_PADDING = 48;
const LABEL_HEIGHT = 22;

export const DEVICE_PRESETS = [
  { id: 'mobile', label: 'Mobile', width: 390, height: 844 },
  { id: 'tablet', label: 'Tablet', width: 820, height: 1180 },
  { id: 'desktop', label: 'Desktop', width: 1440, height: 900 },
];

function clampScale(v) {
  return Math.min(CANVAS_MAX_SCALE, Math.max(CANVAS_MIN_SCALE, v));
}

function layoutFrames(frames) {
  let x = 0;
  const placed = frames.map((f) => {
    const item = { ...f, x, y: 0 };
    x += f.width + FRAME_GAP;
    return item;
  });
  const width = placed.length ? x - FRAME_GAP : 0;
  const height = placed.reduce((m, f) => Math.max(m, f.height), 0);
  return { placed, width, height };
}

/**
 * Owns the pan/zoom viewport state for a Figma-style preview canvas.
 */
export function useCanvasView(frames) {
  const viewportRef = useRef(null);
  const [view, setView] = useState({ scale: 1, tx: 0, ty: 0 });
  const [box, setBox] = useState({ w: 0, h: 0 });
  const touchedRef = useRef(false);

  const layout = useMemo(() => layoutFrames(frames), [frames]);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  const fitInto = useCallback((w, h) => {
    const { width, height } = layoutRef.current;
    if (!width || !height || w <= 0 || h <= 0) return;
    const availW = Math.max(1, w - FIT_PADDING * 2);
    const availH = Math.max(1, h - FIT_PADDING * 2 - LABEL_HEIGHT);
    const scale = clampScale(Math.min(availW / width, availH / height, 1));
    setView({
      scale,
      tx: (w - width * scale) / 2,
      ty: Math.max(FIT_PADDING, (h - height * scale) / 2),
    });
  }, []);

  const fitNow = useCallback(() => {
    touchedRef.current = false;
    const el = viewportRef.current;
    if (el) fitInto(el.clientWidth, el.clientHeight);
  }, [fitInto]);

  useLayoutEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const measure = () => {
      const node = viewportRef.current;
      if (!node) return;
      const w = node.clientWidth;
      const h = node.clientHeight;
      if (w <= 0 || h <= 0) return;
      setBox((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
      if (!touchedRef.current) fitInto(w, h);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [fitInto]);

  // Re-fit whenever the artboard set changes shape, unless the user has
  // taken manual control of the viewport.
  const signature = layout.placed.map((f) => `${f.id}:${f.width}x${f.height}`).join('|');
  useEffect(() => {
    if (touchedRef.current) return;
    fitNow();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);

  const zoomAt = useCallback((nextScale, px, py) => {
    touchedRef.current = true;
    setView((prev) => {
      const scale = clampScale(nextScale);
      if (scale === prev.scale) return prev;
      const ratio = scale / prev.scale;
      return { scale, tx: px - (px - prev.tx) * ratio, ty: py - (py - prev.ty) * ratio };
    });
  }, []);

  const zoomBy = useCallback(
    (factor) => {
      const el = viewportRef.current;
      const cx = el ? el.clientWidth / 2 : 0;
      const cy = el ? el.clientHeight / 2 : 0;
      setView((prev) => {
        touchedRef.current = true;
        const scale = clampScale(prev.scale * factor);
        if (scale === prev.scale) return prev;
        const ratio = scale / prev.scale;
        return { scale, tx: cx - (cx - prev.tx) * ratio, ty: cy - (cy - prev.ty) * ratio };
      });
    },
    [],
  );

  const setScale = useCallback(
    (next) => {
      zoomBy(clampScale(next) / (view.scale || 1));
    },
    [zoomBy, view.scale],
  );

  const panBy = useCallback((dx, dy) => {
    touchedRef.current = true;
    setView((prev) => ({ ...prev, tx: prev.tx + dx, ty: prev.ty + dy }));
  }, []);

  return { viewportRef, view, box, layout, zoomAt, zoomBy, setScale, panBy, fitNow };
}

/**
 * Renders artboards on an infinite pannable / zoomable canvas.
 */
export function PreviewCanvas({ canvas, renderFrame, className }) {
  const { viewportRef, view, layout, zoomAt, panBy } = canvas;
  const [spaceHeld, setSpaceHeld] = useState(false);
  const [modHeld, setModHeld] = useState(false);
  const [panning, setPanning] = useState(false);
  const panRef = useRef(null);

  useEffect(() => {
    const down = (e) => {
      if (e.ctrlKey || e.metaKey) setModHeld(true);
      if (e.code === 'Space' && !e.repeat) {
        const t = e.target;
        const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
        if (!typing) setSpaceHeld(true);
      }
    };
    const up = (e) => {
      if (!e.ctrlKey && !e.metaKey) setModHeld(false);
      if (e.code === 'Space') setSpaceHeld(false);
    };
    const clear = () => {
      setSpaceHeld(false);
      setModHeld(false);
    };
    window.addEventListener('keydown', down);
    window.addEventListener('keyup', up);
    window.addEventListener('blur', clear);
    return () => {
      window.removeEventListener('keydown', down);
      window.removeEventListener('keyup', up);
      window.removeEventListener('blur', clear);
    };
  }, []);

  // Non-passive so preventDefault sticks: WebView2 otherwise treats
  // Ctrl+wheel as a host page-zoom and shrinks the whole IDE.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const onWheel = (e) => {
      e.preventDefault();
      e.stopPropagation();
      const rect = el.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) {
        zoomAt(view.scale * Math.exp(-e.deltaY / 400), e.clientX - rect.left, e.clientY - rect.top);
      } else {
        panBy(-e.deltaX, -e.deltaY);
      }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [viewportRef, view.scale, zoomAt, panBy]);

  const onPointerDown = (e) => {
    const onBackground = e.target === e.currentTarget || e.target.dataset?.canvasBackground === 'true';
    const wantsPan = e.button === 1 || spaceHeld || (e.button === 0 && onBackground);
    if (!wantsPan) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    panRef.current = { x: e.clientX, y: e.clientY };
    setPanning(true);
  };

  const onPointerMove = (e) => {
    if (!panRef.current) return;
    const dx = e.clientX - panRef.current.x;
    const dy = e.clientY - panRef.current.y;
    panRef.current = { x: e.clientX, y: e.clientY };
    panBy(dx, dy);
  };

  const endPan = (e) => {
    if (!panRef.current) return;
    panRef.current = null;
    setPanning(false);
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // Pointer was already released.
    }
  };

  // Frame content lives in opaque-origin iframes, which swallow every wheel
  // and pointer event before the host sees it. Cover the canvas for exactly
  // as long as a pan / zoom modifier is held; the rest of the time the
  // rendered page stays fully interactive.
  const captureActive = spaceHeld || modHeld || panning;

  return (
    <div
      ref={viewportRef}
      data-canvas-background="true"
      className={cn(
        'preview-checkerboard relative flex-1 overflow-hidden',
        panning ? 'cursor-grabbing' : spaceHeld ? 'cursor-grab' : 'cursor-default',
        className,
      )}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endPan}
      onPointerCancel={endPan}
    >
      <div
        className="absolute left-0 top-0 origin-top-left"
        style={{
          width: layout.width || 1,
          height: layout.height || 1,
          transform: `translate3d(${view.tx}px, ${view.ty}px, 0) scale(${view.scale})`,
        }}
      >
        {layout.placed.map((frame) => (
          <div
            key={frame.id}
            className="absolute"
            style={{ left: frame.x, top: frame.y, width: frame.width, height: frame.height }}
          >
            <div
              className="absolute left-0 flex items-center gap-2 whitespace-nowrap text-muted-foreground"
              style={{
                bottom: '100%',
                marginBottom: 6 / view.scale,
                fontSize: 12 / view.scale,
                lineHeight: `${16 / view.scale}px`,
              }}
            >
              <span className="font-medium">{frame.label}</span>
              <span className="opacity-60">
                {Math.round(frame.width)} × {Math.round(frame.height)}
              </span>
            </div>
            <div className="h-full w-full overflow-hidden rounded-sm border border-border bg-white shadow-lg">
              {renderFrame(frame)}
            </div>
          </div>
        ))}
      </div>
      <div
        className="absolute inset-0"
        style={{ pointerEvents: captureActive ? 'auto' : 'none' }}
        aria-hidden
      />
    </div>
  );
}

/**
 * Zoom readout plus artboard toggles and a custom width × height editor.
 */
export function CanvasControls({
  canvas,
  activeIds,
  onToggle,
  custom,
  onCustomChange,
  presets = DEVICE_PRESETS,
}) {
  const pct = Math.round(canvas.view.scale * 100);
  const [draft, setDraft] = useState({ w: String(custom?.width ?? 1280), h: String(custom?.height ?? 720) });

  const commit = (next) => {
    const width = Math.max(64, Math.min(8000, Number(next.w) || 0));
    const height = Math.max(64, Math.min(8000, Number(next.h) || 0));
    onCustomChange({ width, height });
  };

  return (
    <div className="flex min-w-0 items-center gap-1">
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={() => canvas.zoomBy(1 / 1.2)}
        aria-label="Zoom out"
        title="Zoom out"
      >
        <ZoomOut />
      </Button>
      <span className="w-12 text-center text-xs tabular-nums text-muted-foreground">{pct}%</span>
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={() => canvas.zoomBy(1.2)}
        aria-label="Zoom in"
        title="Zoom in"
      >
        <ZoomIn />
      </Button>
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={canvas.fitNow}
        aria-label="Zoom to fit"
        title="Zoom to fit"
      >
        <Maximize2 />
      </Button>

      <div className="mx-1 h-4 w-px shrink-0 bg-border" />

      {presets.map((p) => (
        <Button
          key={p.id}
          size="xs"
          variant={activeIds.includes(p.id) ? 'secondary' : 'ghost'}
          onClick={() => onToggle(p.id)}
          title={`${p.label} — ${p.width} × ${p.height}`}
        >
          {p.label}
        </Button>
      ))}

      <Popover>
        <PopoverTrigger asChild>
          <Button
            size="xs"
            variant={activeIds.includes('custom') ? 'secondary' : 'ghost'}
            title="Custom size"
          >
            <Monitor className="mr-1" />
            Custom
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-64 space-y-3">
          <div className="flex items-end gap-2">
            <label className="flex-1 space-y-1 text-xs text-muted-foreground">
              Width
              <Input
                type="number"
                value={draft.w}
                onChange={(e) => setDraft((d) => ({ ...d, w: e.target.value }))}
                onBlur={() => commit(draft)}
                onKeyDown={(e) => e.key === 'Enter' && commit(draft)}
                className="h-7"
              />
            </label>
            <label className="flex-1 space-y-1 text-xs text-muted-foreground">
              Height
              <Input
                type="number"
                value={draft.h}
                onChange={(e) => setDraft((d) => ({ ...d, h: e.target.value }))}
                onBlur={() => commit(draft)}
                onKeyDown={(e) => e.key === 'Enter' && commit(draft)}
                className="h-7"
              />
            </label>
            <Button
              size="icon-xs"
              variant="ghost"
              title="Swap orientation"
              onClick={() => {
                const next = { w: draft.h, h: draft.w };
                setDraft(next);
                commit(next);
              }}
            >
              <RotateCw />
            </Button>
          </div>
          <Button
            size="xs"
            variant={activeIds.includes('custom') ? 'secondary' : 'outline'}
            className="w-full"
            onClick={() => {
              commit(draft);
              onToggle('custom');
            }}
          >
            {activeIds.includes('custom') ? 'Hide custom artboard' : 'Show custom artboard'}
          </Button>
        </PopoverContent>
      </Popover>
    </div>
  );
}
