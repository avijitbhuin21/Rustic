import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ZoomIn, ZoomOut, Maximize2 } from 'lucide-react';
import { Button } from '@/components/ui/button';

// One pan/zoom engine for image, PDF, DOCX and HTML previews.
//
// Why: zooming used to go through React state — each wheel tick re-rendered
// and re-laid-out the content, then corrected the scroll position a frame
// later, which reads as jitter. Here the view is a single GPU transform
// (translate3d + scale, origin top-left) written straight to the DOM once
// per animation frame; React only re-renders the % readout.
//
// Interaction:
//   paged documents (`wheel: 'scroll'`, PDF): wheel scrolls, pinch or
//   Cmd/Ctrl+wheel zooms around the cursor; everything else below is shared
//   wheel / trackpad pinch → zoom around the cursor
//   Shift+wheel → pan vertically · horizontal swipe → pan horizontally
//   left-drag (where `canPan` allows) or middle-drag → pan
//   two-finger touch pinch → zoom · double-click → fit
//   keys: arrows / PageUp / PageDown / Home / End pan, + − zoom, 0 fit

const SETTLE_MS = 160;
const EDGE = 48; // px of content kept on screen when panning past an edge

/** Wheel delta in pixels regardless of the device's delta mode. */
function wheelPixels(e, page) {
  if (e.deltaMode === 1) return { dx: e.deltaX * 16, dy: e.deltaY * 16 };
  if (e.deltaMode === 2) return { dx: e.deltaX * page, dy: e.deltaY * page };
  return { dx: e.deltaX, dy: e.deltaY };
}

/**
 * Pan/zoom state for a viewport (overflow hidden) and a content element
 * positioned at its top-left. `fit` is 'contain' (whole thing visible) or
 * 'width' (fill the width, top-aligned — documents). `onSettle` fires after a
 * gesture ends with the current {x, y, k}. `canPan(e)` decides whether a left
 * button press should start a pan (return false to let text selection or an
 * editing tool have the drag).
 */
export function usePanZoom({
  fit: fitMode = 'contain',
  minScale = 0.02,
  maxScale = 40,
  padding = 24,
  onSettle,
  canPan,
  // 'zoom': wheel zooms (images, canvases). 'scroll': wheel scrolls and only
  // pinch or Cmd/Ctrl+wheel zooms (paged documents).
  wheel = 'zoom',
  // Keep the content on its own GPU layer permanently. Right for images
  // (promoting a huge bitmap at gesture start costs a long frame); wrong for
  // text, which only re-rasterises crisply once `will-change` is dropped.
  keepLayer = false,
} = {}) {
  const viewportRef = useRef(null);
  const contentRef = useRef(null);
  const view = useRef({ x: 0, y: 0, k: 1 });
  const size = useRef({ w: 0, h: 0 });
  const touched = useRef(false);
  const raf = useRef(0);
  const settleTimer = useRef(0);
  const opts = useRef({});
  opts.current = { fitMode, minScale, maxScale, padding, onSettle, canPan, keepLayer, wheel };
  const [scale, setScale] = useState(1);
  const [fitScale, setFitScale] = useState(1);
  const [dragging, setDragging] = useState(false);
  // Callers often render a loading state first, so the viewport element can
  // appear (or change) after mount — wiring must follow the element itself.
  const [vpEl, setVpEl] = useState(null);
  useLayoutEffect(() => {
    if (viewportRef.current !== vpEl) setVpEl(viewportRef.current);
  });

  const vpSize = () => {
    const el = viewportRef.current;
    return el ? { vw: el.clientWidth, vh: el.clientHeight } : { vw: 0, vh: 0 };
  };

  /** Keeps content reachable: centered on an axis where it fits, otherwise edges stay within EDGE. */
  const clamp = (v) => {
    const { vw, vh } = vpSize();
    const cw = size.current.w * v.k;
    const ch = size.current.h * v.k;
    const axis = (pos, c, vp) => {
      if (c <= vp) return Math.min(Math.max(pos, 0), vp - c);
      return Math.min(EDGE, Math.max(vp - c - EDGE, pos));
    };
    return { k: v.k, x: axis(v.x, cw, vw), y: axis(v.y, ch, vh) };
  };

  const paint = () => {
    raf.current = 0;
    const el = contentRef.current;
    if (!el) return;
    const v = view.current;
    el.style.transformOrigin = '0 0';
    el.style.transform = `translate3d(${v.x}px, ${v.y}px, 0) scale(${v.k})`;
    setScale(v.k);
  };

  const schedule = () => {
    const el = contentRef.current;
    if (el) el.style.willChange = 'transform';
    if (!raf.current) raf.current = requestAnimationFrame(paint);
    clearTimeout(settleTimer.current);
    settleTimer.current = setTimeout(() => {
      if (contentRef.current && !opts.current.keepLayer) contentRef.current.style.willChange = '';
      opts.current.onSettle?.({ ...view.current });
    }, SETTLE_MS);
  };

  const set = useCallback((next, { user = true } = {}) => {
    if (user) touched.current = true;
    view.current = clamp(next);
    schedule();
  }, []);

  /** Scale that fits the content per the fit mode. */
  const computeFit = () => {
    const { vw, vh } = vpSize();
    const { w, h } = size.current;
    const p = opts.current.padding;
    if (!w || !h || !vw || !vh) return null;
    const kw = (vw - 2 * p) / w;
    const k = opts.current.fitMode === 'width' ? kw : Math.min(kw, (vh - 2 * p) / h);
    return Math.max(opts.current.minScale, Math.min(opts.current.maxScale, k));
  };

  const fit = useCallback(() => {
    const k = computeFit();
    if (k == null) return;
    const { vw, vh } = vpSize();
    const { w, h } = size.current;
    touched.current = false;
    setFitScale(k);
    const y = opts.current.fitMode === 'width' ? opts.current.padding : (vh - h * k) / 2;
    view.current = clamp({ k, x: (vw - w * k) / 2, y });
    schedule();
  }, []);

  /** Zoom by `factor` around viewport point (cx, cy); defaults to the center. */
  const zoomAt = useCallback((factor, cx, cy) => {
    const { vw, vh } = vpSize();
    const v = view.current;
    const k = Math.min(opts.current.maxScale, Math.max(opts.current.minScale, v.k * factor));
    const f = k / v.k;
    const ax = cx ?? vw / 2;
    const ay = cy ?? vh / 2;
    set({ k, x: ax - (ax - v.x) * f, y: ay - (ay - v.y) * f });
  }, [set]);

  const panBy = useCallback((dx, dy) => {
    const v = view.current;
    set({ k: v.k, x: v.x + dx, y: v.y + dy });
  }, [set]);

  /** Absolute zoom (as used by +/- buttons), anchored at the viewport center. */
  const zoomTo = useCallback((k) => zoomAt(k / view.current.k), [zoomAt]);

  /**
   * The content's layout just grew by `factor` (e.g. the PDF re-rendered at a
   * higher resolution): divide the transform scale so nothing moves on screen.
   */
  const rebase = useCallback((factor) => {
    if (!factor || factor === 1) return;
    view.current = { ...view.current, k: view.current.k / factor };
    setFitScale((f) => f / factor);
    paint();
  }, []);

  /** Tells the engine the content's untransformed size; re-fits until the user zooms or pans. */
  const setContentSize = useCallback((w, h) => {
    const changed = w !== size.current.w || h !== size.current.h;
    size.current = { w, h };
    if (!changed) return;
    const k = computeFit();
    if (k != null) setFitScale(k);
    if (!touched.current) fit();
    else set(view.current, { user: false });
  }, [fit, set]);

  // Wheel, pointer, keyboard and resize wiring.
  useLayoutEffect(() => {
    const vp = vpEl;
    if (!vp) return undefined;

    const onWheel = (e) => {
      e.preventDefault();
      e.stopPropagation();
      const rect = vp.getBoundingClientRect();
      const { dx, dy } = wheelPixels(e, rect.height);
      if (opts.current.wheel === 'scroll') {
        if (!(e.ctrlKey || e.metaKey)) {
          // Shift turns a vertical mouse wheel into a horizontal scroll.
          if (e.shiftKey && !dx) panBy(-dy, 0);
          else panBy(-dx, -dy);
          return;
        }
        // Pinch sends small ctrl+wheel deltas; a mouse wheel notch sends ~100, so cap the step.
        const step = -dy * (e.ctrlKey ? 0.012 : 0.0022);
        zoomAt(Math.exp(Math.max(-0.25, Math.min(0.25, step))), e.clientX - rect.left, e.clientY - rect.top);
        return;
      }
      if (e.shiftKey) {
        panBy(-(dx || 0), -dy);
        return;
      }
      if (Math.abs(dx) > Math.abs(dy) && !e.ctrlKey) {
        panBy(-dx, 0);
        return;
      }
      // Pinch arrives as ctrl+wheel with small deltas; scale it up a bit.
      const factor = Math.exp(-dy * (e.ctrlKey ? 0.012 : 0.0022));
      zoomAt(factor, e.clientX - rect.left, e.clientY - rect.top);
    };

    const pointers = new Map();
    let drag = null;
    let pinch = null;
    const onDown = (e) => {
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        pinch = { d: Math.hypot(a.x - b.x, a.y - b.y), k: view.current.k };
        drag = null;
        return;
      }
      const allow = e.button === 1 || (e.button === 0 && (opts.current.canPan ? opts.current.canPan(e) : true));
      if (!allow) return;
      drag = { id: e.pointerId, sx: e.clientX, sy: e.clientY, vx: view.current.x, vy: view.current.y, moved: false };
    };
    const onMove = (e) => {
      if (!pointers.has(e.pointerId)) return;
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pinch && pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        const rect = vp.getBoundingClientRect();
        const target = pinch.k * (Math.hypot(a.x - b.x, a.y - b.y) / pinch.d);
        zoomAt(target / view.current.k, (a.x + b.x) / 2 - rect.left, (a.y + b.y) / 2 - rect.top);
        return;
      }
      if (!drag || drag.id !== e.pointerId) return;
      const dx = e.clientX - drag.sx;
      const dy = e.clientY - drag.sy;
      if (!drag.moved && Math.hypot(dx, dy) < 3) return;
      if (!drag.moved) {
        drag.moved = true;
        try { vp.setPointerCapture(e.pointerId); } catch { /* not capturable */ }
        setDragging(true);
        window.getSelection?.()?.removeAllRanges();
      }
      e.preventDefault();
      set({ k: view.current.k, x: drag.vx + dx, y: drag.vy + dy });
    };
    const onUp = (e) => {
      pointers.delete(e.pointerId);
      if (pointers.size < 2) pinch = null;
      if (drag && drag.id === e.pointerId) {
        if (drag.moved) {
          // Swallow the click that would otherwise follow a drag.
          const swallow = (ev) => { ev.stopPropagation(); ev.preventDefault(); };
          vp.addEventListener('click', swallow, { capture: true, once: true });
          setTimeout(() => vp.removeEventListener('click', swallow, { capture: true }), 0);
        }
        drag = null;
        setDragging(false);
      }
    };
    const onDbl = (e) => {
      if (opts.current.canPan && !opts.current.canPan(e)) return;
      fit();
    };
    const onKey = (e) => {
      if (e.target !== vp) return;
      const { vh } = vpSize();
      const step = 60;
      const map = {
        ArrowUp: () => panBy(0, step),
        ArrowDown: () => panBy(0, -step),
        ArrowLeft: () => panBy(step, 0),
        ArrowRight: () => panBy(-step, 0),
        PageUp: () => panBy(0, vh * 0.9),
        PageDown: () => panBy(0, -vh * 0.9),
        ' ': () => panBy(0, e.shiftKey ? vh * 0.9 : -vh * 0.9),
        Home: () => set({ ...view.current, y: Number.MAX_SAFE_INTEGER }),
        End: () => set({ ...view.current, y: -Number.MAX_SAFE_INTEGER }),
        '+': () => zoomAt(1.2),
        '=': () => zoomAt(1.2),
        '-': () => zoomAt(1 / 1.2),
        0: () => fit(),
      };
      const fn = map[e.key];
      if (fn) {
        e.preventDefault();
        fn();
      }
    };
    const ro = new ResizeObserver(() => {
      const k = computeFit();
      if (k != null) setFitScale(k);
      if (!touched.current) fit();
      else set(view.current, { user: false });
    });

    vp.addEventListener('wheel', onWheel, { passive: false });
    vp.addEventListener('pointerdown', onDown);
    vp.addEventListener('pointermove', onMove);
    vp.addEventListener('pointerup', onUp);
    vp.addEventListener('pointercancel', onUp);
    vp.addEventListener('dblclick', onDbl);
    vp.addEventListener('keydown', onKey);
    ro.observe(vp);
    return () => {
      vp.removeEventListener('wheel', onWheel);
      vp.removeEventListener('pointerdown', onDown);
      vp.removeEventListener('pointermove', onMove);
      vp.removeEventListener('pointerup', onUp);
      vp.removeEventListener('pointercancel', onUp);
      vp.removeEventListener('dblclick', onDbl);
      vp.removeEventListener('keydown', onKey);
      ro.disconnect();
      cancelAnimationFrame(raf.current);
      raf.current = 0;
      clearTimeout(settleTimer.current);
    };
  }, [vpEl, fit, panBy, set, zoomAt]);

  return {
    viewportRef,
    contentRef,
    scale,
    fitScale,
    dragging,
    fit,
    zoomAt,
    zoomTo,
    panBy,
    rebase,
    setContentSize,
    setView: (partial) => set({ ...view.current, ...partial }),
    getView: () => ({ ...view.current }),
  };
}

/** Class names for a pan/zoom viewport: clipped, focusable, grab cursor. */
export function panViewportClass(dragging, extra = '') {
  return `relative h-full w-full touch-none select-none overflow-hidden outline-none ${dragging ? 'cursor-grabbing' : 'cursor-grab'} ${extra}`;
}

/** Zoom out / % (relative to fit) / zoom in / fit buttons bound to a `usePanZoom` instance. */
export function PanZoomControls({ pz, fitLabel = 'Fit' }) {
  const pct = Math.round((pz.scale / (pz.fitScale || 1)) * 100);
  return (
    <div className="flex items-center gap-1">
      <Button size="icon-xs" variant="ghost" onClick={() => pz.zoomAt(1 / 1.25)} aria-label="Zoom out" title="Zoom out (−)">
        <ZoomOut />
      </Button>
      <span className="w-12 text-center text-xs tabular-nums text-muted-foreground">{pct}%</span>
      <Button size="icon-xs" variant="ghost" onClick={() => pz.zoomAt(1.25)} aria-label="Zoom in" title="Zoom in (+)">
        <ZoomIn />
      </Button>
      <Button size="icon-xs" variant="ghost" onClick={pz.fit} aria-label={fitLabel} title={`${fitLabel} (0 / double-click)`}>
        <Maximize2 />
      </Button>
    </div>
  );
}

/**
 * Toolbar strip + clipped, focusable pan/zoom viewport. `children` go inside
 * the transformed content box (positioned at the top-left, untransformed size
 * reported by the caller through `pz.setContentSize`).
 */
export function PanZoomSurface({ pz, toolbar, children, background = 'preview-checkerboard', contentClassName = '' }) {
  return (
    <div className="flex h-full w-full flex-col">
      {toolbar && (
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-border bg-muted/20 px-2">
          {toolbar}
        </div>
      )}
      <div
        ref={pz.viewportRef}
        tabIndex={0}
        className={panViewportClass(pz.dragging, `min-h-0 flex-1 ${background}`)}
      >
        <div ref={pz.contentRef} className={`absolute left-0 top-0 ${contentClassName}`}>
          {children}
        </div>
      </div>
    </div>
  );
}
