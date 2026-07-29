import React, { useEffect, useLayoutEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { dirname, handleMarkdownLinkClick } from '@/lib/markdown-assets';
import DOMPurify from 'dompurify';
import { Skeleton } from '@/components/ui/skeleton';
import { basename } from '@/state/editor';
import { PreviewSurface } from './preview-surface';
import { ZoomControls, ToolbarToggleGap, useFitZoom } from './preview-zoom';

const MIN_SCALE = 0.05;
const MAX_SCALE = 16;
// Padding kept around the artwork when fitting it to the pane.
const FIT_MARGIN = 32;
// The SVG spec's default intrinsic size, used when a document declares
// neither a viewBox nor a measurable bounding box.
const FALLBACK_SIZE = { w: 300, h: 150 };

/** Reads an <svg> element's intrinsic size, preferring its viewBox. */
function naturalSize(svg) {
  const vb = svg.viewBox?.baseVal;
  if (vb && vb.width > 0 && vb.height > 0) return { w: vb.width, h: vb.height };
  try {
    const bb = svg.getBBox();
    if (bb.width > 0 && bb.height > 0) return { w: bb.width, h: bb.height };
  } catch {
    // getBBox throws on a detached / unrendered node.
  }
  return FALLBACK_SIZE;
}

// Pure rendered preview. Editing is handled by the Monaco editor via the
// shared Edit ⇄ Preview toggle in editor-pane.jsx — this component used to
// carry its own Preview/Edit toolbar + SourceCodeEditor, which duplicated
// that control. It now renders the SVG only (Ctrl+wheel zoom preserved).
export default function SvgPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);
  const [natural, setNatural] = useState(null);
  const previewRef = useRef(null);
  const surfaceRef = useRef(null);

  const reloadVersion = useFileReloadVersion(tab.path);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setText(null);
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (!cancelled) setText(c ?? '');
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path, reloadVersion]);

  const safe = useMemo(
    () =>
      DOMPurify.sanitize(text ?? '', { USE_PROFILES: { svg: true, svgFilters: true } }),
    [text],
  );

  // Measure the artwork once it's in the DOM. Driving the layout box off the
  // intrinsic size (rather than letting the SVG size itself) is what lets the
  // scroll container reserve space for a zoomed-up drawing.
  useLayoutEffect(() => {
    const host = previewRef.current;
    const svg = host?.querySelector('svg');
    setNatural(svg ? naturalSize(svg) : null);
  }, [safe]);

  const { scale, setScale, fitScale, fitNow } = useFitZoom(
    surfaceRef,
    ({ w, h }) => {
      if (!natural) return 0;
      const availW = w - FIT_MARGIN;
      const availH = h - FIT_MARGIN;
      if (availW <= 0 || availH <= 0) return 0;
      return Math.max(MIN_SCALE, Math.min(availW / natural.w, availH / natural.h, 1));
    },
    [natural],
  );

  // Intercept link clicks in the SVG preview via the shared handler (it
  // checks both href and xlink:href, allow-lists external schemes, and opens
  // local paths relative to this file in an editor tab).
  useEffect(() => {
    const baseDir = dirname(tab.path);
    const handleClick = (e) => handleMarkdownLinkClick(e, baseDir);

    const el = previewRef.current;
    if (el) {
      el.addEventListener('click', handleClick);
      return () => el.removeEventListener('click', handleClick);
    }
  }, [safe, tab.path]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }
  if (text == null) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-64 w-64" />
      </div>
    );
  }

  const toolbar = (
    <>
      <div className="flex min-w-0 items-center gap-1">
        <ZoomControls
          scale={scale}
          fitScale={fitScale}
          onScaleChange={setScale}
          minScale={MIN_SCALE}
          maxScale={MAX_SCALE}
          onFit={fitNow}
          fitLabel="Fit to pane"
        />
        <span className="ml-1 truncate text-xs text-muted-foreground">
          {basename(tab.path)}
          {natural && (
            <span className="ml-2 text-muted-foreground/60">
              {Math.round(natural.w)} × {Math.round(natural.h)}
            </span>
          )}
        </span>
      </div>
      <ToolbarToggleGap />
    </>
  );

  return (
    <PreviewSurface
      toolbar={toolbar}
      scale={scale}
      onScaleChange={setScale}
      minScale={MIN_SCALE}
      maxScale={MAX_SCALE}
      scrollRef={surfaceRef}
    >
      {/* Same reasoning as image-preview: `justify-center` would strand the
          overflow of a zoomed-up child past the scroll container's start edge
          where it can never be scrolled into view. Auto margins centre it
          while it fits, and a top-left transform origin keeps every scaled
          pixel inside the reachable (end-direction) overflow region. */}
      <div className="flex min-h-full w-full p-4">
        <div
          style={{
            width: natural ? Math.max(1, Math.floor(natural.w * scale)) : undefined,
            height: natural ? Math.max(1, Math.floor(natural.h * scale)) : undefined,
            margin: 'auto',
          }}
        >
          {/* The outer box owns the scaled layout size; this inner node stays
              at the intrinsic size and is transformed. Scaling by transform
              rather than sizing the <svg> keeps documents that declare no
              viewBox — and so can't rescale themselves — working. */}
          <div
            ref={previewRef}
            style={{
              width: natural?.w,
              height: natural?.h,
              transform: `scale(${scale})`,
              transformOrigin: 'top left',
            }}
            className="[&>svg]:block [&>svg]:h-full [&>svg]:w-full [&>svg]:max-h-none [&>svg]:max-w-none"
            dangerouslySetInnerHTML={{ __html: safe }}
          />
        </div>
      </div>
    </PreviewSurface>
  );
}
