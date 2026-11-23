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
 * `wheel: 'scroll'` makes the wheel scroll (Cmd/Ctrl+wheel or pinch zooms) and keeps the
 * artboards in view, like the PDF viewer; `fit: 'width'` fits to width, top-aligned.
 */
export function useCanvasView(frames, { wheel = 'zoom', fit = 'contain' } = {}) {
  // The canvas often mounts after a loading skeleton, so effects follow the element, not mount.
  const elRef = useRef(null);
  const [viewportEl, setViewportEl] = useState(null);
  const viewportRef = useCallback((node) => {
    elRef.current = node;
    setViewportEl(node);
  }, []);
  const [view, setViewState] = useState({ scale: 1, tx: 0, ty: 0 });
  const viewRef = useRef(view);
  viewRef.current = view;
  const [box, setBox] = useState({ w: 0, h: 0 });
  const touchedRef = useRef(false);
  const scrollMode = wheel === 'scroll';

  const layout = useMemo(() => layoutFrames(frames), [frames]);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  /** In scroll mode, keep the artboards from being scrolled out of the viewport. */
  const bound = useCallback(
    (v) => {
      const el = elRef.current;
      if (!scrollMode || !el) return v;
      const span = (t, size, room) => {
        let lo = room - FIT_PADDING - size;
        let hi = FIT_PADDING;
        if (lo > hi) [lo, hi] = [hi, lo];
        return Math.min(hi, Math.max(lo, t));
      };
      const { width, height } = layoutRef.current;
      return {
        ...v,
        tx: span(v.tx, width * v.scale, el.clientWidth),
        ty: span(v.ty, height * v.scale, el.clientHeight),
      };
    },
    [scrollMode],
  );

  const setView = useCallback(
    (update) => {
      setViewState((prev) => {
        const next = typeof update === 'function' ? update(prev) : update;
        return next === prev ? prev : bound(next);
      });
    },
    [bound],
  );

  const fitInto = useCallback(
    (w, h) => {
      const { width, height } = layoutRef.current;
      if (!width || !height || w <= 0 || h <= 0) return;
      const availW = Math.max(1, w - FIT_PADDING * 2);
      if (fit === 'width') {
        const scale = clampScale(Math.min(availW / width, 1));
        setViewState({ scale, tx: (w - width * scale) / 2, ty: FIT_PADDING });
        return;
      }
      const availH = Math.max(1, h - FIT_PADDING * 2 - LABEL_HEIGHT);
      const scale = clampScale(Math.min(availW / width, availH / height, 1));
      setViewState({
        scale,
        tx: (w - width * scale) / 2,
        ty: Math.max(FIT_PADDING, (h - height * scale) / 2),
      });
    },
    [fit],
  );

  const fitNow = useCallback(() => {
    touchedRef.current = false;
    const el = elRef.current;
    if (el) fitInto(el.clientWidth, el.clientHeight);
  }, [fitInto]);

  useLayoutEffect(() => {
    const el = viewportEl;
    if (!el) return undefined;
    const measure = () => {
      const w = el.clientWidth;
      const h = el.clientHeight;
      if (w <= 0 || h <= 0) return;
      setBox((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
      if (!touchedRef.current) fitInto(w, h);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [viewportEl, fitInto]);

  // Re-fit whenever the artboard set changes shape, unless the user has
  // taken manual control of the viewport.
  const signature = layout.placed.map((f) => `${f.id}:${f.width}x${f.height}`).join('|');
  useEffect(() => {
    if (touchedRef.current) return;
    fitNow();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);

  const zoomAt = useCallback(
    (nextScale, px, py) => {
      touchedRef.current = true;
      setView((prev) => {
        const scale = clampScale(nextScale);
        if (scale === prev.scale) return prev;
        const ratio = scale / prev.scale;
        return { scale, tx: px - (px - prev.tx) * ratio, ty: py - (py - prev.ty) * ratio };
      });
    },
    [setView],
  );

  const zoomBy = useCallback(
    (factor) => {
      const el = elRef.current;
      const cx = el ? el.clientWidth / 2 : 0;
      const cy = el ? el.clientHeight / 2 : 0;
      touchedRef.current = true;
      setView((prev) => {
        const scale = clampScale(prev.scale * factor);
        if (scale === prev.scale) return prev;
        const ratio = scale / prev.scale;
        return { scale, tx: cx - (cx - prev.tx) * ratio, ty: cy - (cy - prev.ty) * ratio };
      });
    },
    [setView],
  );

  const setScale = useCallback(
    (next) => {
      zoomBy(clampScale(next) / (view.scale || 1));
    },
    [zoomBy, view.scale],
  );

  const panBy = useCallback(
    (dx, dy) => {
      touchedRef.current = true;
      setView((prev) => ({ ...prev, tx: prev.tx + dx, ty: prev.ty + dy }));
    },
    [setView],
  );

  // Wheel input, batched to one update per frame so a fast trackpad stream doesn't
  // re-render per event (that read as jitter).
  const pendingRef = useRef(null);
  const rafRef = useRef(0);
  useEffect(() => () => cancelAnimationFrame(rafRef.current), []);
  const flushWheel = useCallback(() => {
    rafRef.current = 0;
    const p = pendingRef.current;
    pendingRef.current = null;
    if (!p) return;
    touchedRef.current = true;
    setView((prev) => {
      let v = prev;
      if (p.zoom !== 1) {
        const scale = clampScale(prev.scale * p.zoom);
        const r = scale / prev.scale;
        v = { scale, tx: p.cx - (p.cx - prev.tx) * r, ty: p.cy - (p.cy - prev.ty) * r };
      }
      if (p.dx || p.dy) v = { ...v, tx: v.tx + p.dx, ty: v.ty + p.dy };
      return v;
    });
  }, [setView]);

  /** Apply one wheel event; `cx`/`cy` are viewport coordinates of the pointer. */
  const wheelInput = useCallback(
    ({ dx = 0, dy = 0, ctrl = false, meta = false, shift = false, cx = 0, cy = 0 }) => {
      const p = pendingRef.current || (pendingRef.current = { zoom: 1, dx: 0, dy: 0, cx: 0, cy: 0 });
      const zoomTo = (k) => {
        p.zoom *= Math.exp(-dy * k);
        p.cx = cx;
        p.cy = cy;
      };
      if (scrollMode) {
        // A trackpad pinch arrives as ctrl+wheel with small deltas; a mouse notch is ~100.
        if (ctrl || meta) zoomTo(ctrl && Math.abs(dy) < 50 ? 0.012 : 0.0022);
        else if (shift && !dx) p.dx -= dy;
        else {
          p.dx -= dx;
          p.dy -= dy;
        }
      } else if (shift) {
        p.dy -= dy;
      } else if (Math.abs(dx) > Math.abs(dy) && !ctrl) {
        p.dx -= dx;
      } else {
        zoomTo(ctrl ? 0.012 : 0.0022);
      }
      if (!rafRef.current) rafRef.current = requestAnimationFrame(flushWheel);
    },
    [scrollMode, flushWheel],
  );

  /** Apply a wheel event reported from inside a frame, in that frame's page coordinates. */
  const wheelFromFrame = useCallback(
    (frameId, m) => {
      const frame = layoutRef.current.placed.find((f) => f.id === frameId);
      if (!frame) return;
      const v = viewRef.current;
      wheelInput({
        ...m,
        cx: v.tx + (frame.x + (Number(m.x) || 0)) * v.scale,
        cy: v.ty + (frame.y + (Number(m.y) || 0)) * v.scale,
      });
    },
    [wheelInput],
  );

  /** Scroll so page offset `y` of a frame sits at the top of the viewport. */
  const scrollToFrameY = useCallback(
    (frameId, y) => {
      const frame = layoutRef.current.placed.find((f) => f.id === frameId);
      if (!frame) return;
      touchedRef.current = true;
      setView((prev) => ({ ...prev, ty: FIT_PADDING - (frame.y + y) * prev.scale }));
    },
    [setView],
  );

  return {
    viewportRef,
    viewportEl,
    view,
    box,
    layout,
    zoomAt,
    zoomBy,
    setScale,
    panBy,
    fitNow,
    wheelInput,
    wheelFromFrame,
    scrollToFrameY,
  };
}

/**
 * Renders artboards on an infinite pannable / zoomable canvas.
 */
export function PreviewCanvas({ canvas, renderFrame, className, interactive = false }) {
  const { viewportRef, viewportEl, view, layout, panBy, wheelInput } = canvas;
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
    const el = viewportEl;
    if (!el) return undefined;
    const onWheel = (e) => {
      e.preventDefault();
      e.stopPropagation();
      const rect = el.getBoundingClientRect();
      const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? el.clientHeight : 1;
      wheelInput({
        dx: e.deltaX * k,
        dy: e.deltaY * k,
        ctrl: e.ctrlKey,
        meta: e.metaKey,
        shift: e.shiftKey,
        cx: e.clientX - rect.left,
        cy: e.clientY - rect.top,
      });
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [viewportEl, wheelInput]);

  const onPointerDown = (e) => {
    const onBackground = e.target === e.currentTarget || e.target.dataset?.canvasBackground === 'true';
    const wantsPan = e.button === 1 || spaceHeld || (e.button === 0 && (onBackground || !interactive));
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
  // Opaque-origin iframes swallow every wheel and pointer event before the
  // host sees them. In navigate mode (default) the capture layer stays on so
  // wheel-zoom and drag-pan work over the page too; "Interact" mode hands the
  // page back its events, with Space/Ctrl/Cmd still temporarily capturing.
  const captureActive = !interactive || spaceHeld || modHeld || panning;

  return (
    <div
      ref={viewportRef}
      data-canvas-background="true"
      className={cn(
        'preview-checkerboard relative flex-1 overflow-hidden',
        panning ? 'cursor-grabbing' : spaceHeld || !interactive ? 'cursor-grab' : 'cursor-default',
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
                {Math.round(frame.width)} × {Math.round(frame.viewportHeight ?? frame.height)}
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
