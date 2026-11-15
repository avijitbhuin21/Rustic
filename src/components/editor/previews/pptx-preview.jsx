import React, { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { fetchFileBytes } from '@/lib/file-bytes';
import { basename } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';

/** PowerPoint (.pptx) viewer rendered to canvas by pptxviewjs (MIT, lazy-loaded). */
export default function PptxPreview({ tab }) {
  const canvasRef = useRef(null);
  const wrapRef = useRef(null);
  const viewerRef = useRef(null);
  const [count, setCount] = useState(0);
  const [index, setIndex] = useState(0);
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(true);
  const reloadVersion = useFileReloadVersion(tab.path);

  /** Sizes the canvas to the pane (16:9 box, device-pixel sharp) and draws slide `i`. */
  const draw = useCallback(async (i) => {
    const viewer = viewerRef.current;
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!viewer || !canvas || !wrap) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(320, wrap.clientWidth - 32);
    const h = Math.min(wrap.clientHeight - 32, (w * 9) / 16);
    const width = Math.round(Math.min(w, (h * 16) / 9));
    const height = Math.round((width * 9) / 16);
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    await viewer.renderSlide(i, canvas, { quality: 'high' });
    setIndex(viewer.getCurrentSlideIndex?.() ?? i);
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    (async () => {
      try {
        const [{ PPTXViewer }, bytes] = await Promise.all([
          import('pptxviewjs'),
          fetchFileBytes(tab.path, { version: reloadVersion }),
        ]);
        if (cancelled) return;
        viewerRef.current?.destroy?.();
        const viewer = new PPTXViewer({ canvas: canvasRef.current, slideSizeMode: 'fit' });
        viewerRef.current = viewer;
        await viewer.loadFile(bytes);
        if (cancelled) return;
        setCount(viewer.getSlideCount());
        setLoading(false);
        await draw(0);
      } catch (e) {
        if (!cancelled) {
          setLoading(false);
          setError(`Could not render this presentation (${String(e?.message || e)}).`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tab.path, reloadVersion, draw]);

  useEffect(() => () => viewerRef.current?.destroy?.(), []);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap || typeof ResizeObserver === 'undefined') return undefined;
    let t = null;
    const ro = new ResizeObserver(() => {
      clearTimeout(t);
      t = setTimeout(() => draw(viewerRef.current?.getCurrentSlideIndex?.() ?? 0), 120);
    });
    ro.observe(wrap);
    return () => { clearTimeout(t); ro.disconnect(); };
  }, [draw]);

  const go = (i) => {
    if (i < 0 || i >= count) return;
    draw(i).catch((e) => setError(String(e?.message || e)));
  };

  return (
    <div
      className="flex h-full w-full flex-col outline-none"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === 'ArrowRight' || e.key === 'PageDown' || e.key === ' ') { e.preventDefault(); go(index + 1); }
        if (e.key === 'ArrowLeft' || e.key === 'PageUp') { e.preventDefault(); go(index - 1); }
      }}
    >
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-2">
        <div className="min-w-0 truncate text-xs text-muted-foreground">{basename(tab.path)}</div>
        <div className="ml-auto flex items-center gap-1">
          <Button variant="ghost" size="icon" className="size-7" disabled={index <= 0} onClick={() => go(index - 1)}>
            <ChevronLeft className="size-3.5" />
          </Button>
          <span className="min-w-[4.5rem] text-center text-[11px] text-muted-foreground">
            {count ? `${index + 1} / ${count}` : '–'}
          </span>
          <Button variant="ghost" size="icon" className="size-7" disabled={index >= count - 1} onClick={() => go(index + 1)}>
            <ChevronRight className="size-3.5" />
          </Button>
        </div>
      </div>
      <div ref={wrapRef} className="preview-checkerboard relative flex min-h-0 flex-1 items-center justify-center overflow-auto p-4">
        {loading && !error && <Skeleton className="absolute inset-8" />}
        {error
          ? <div className="text-sm text-destructive">{error}</div>
          : <canvas ref={canvasRef} className="rounded-md bg-background shadow-lg" />}
      </div>
    </div>
  );
}
