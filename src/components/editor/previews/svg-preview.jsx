import React, { useEffect, useLayoutEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { dirname, handleMarkdownLinkClick } from '@/lib/markdown-assets';
import DOMPurify from 'dompurify';
import { Skeleton } from '@/components/ui/skeleton';
import { basename } from '@/state/editor';
import { ToolbarToggleGap } from './preview-zoom';
import { CanvasControls, PreviewCanvas, useCanvasView } from './preview-canvas';

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
  const [customSize, setCustomSize] = useState(null);

  const frames = useMemo(() => {
    const w = customSize?.width ?? natural?.w ?? FALLBACK_SIZE.w;
    const h = customSize?.height ?? natural?.h ?? FALLBACK_SIZE.h;
    return [
      {
        id: 'artboard',
        label: customSize ? 'Custom' : 'Artboard',
        width: Math.max(1, Math.round(w)),
        height: Math.max(1, Math.round(h)),
      },
    ];
  }, [customSize, natural]);

  const canvas = useCanvasView(frames);
  const previewRef = useRef(null);

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
        <CanvasControls
          canvas={canvas}
          activeIds={customSize ? ['custom'] : []}
          onToggle={() => setCustomSize((prev) => (prev ? null : { width: Math.round(natural?.w ?? FALLBACK_SIZE.w), height: Math.round(natural?.h ?? FALLBACK_SIZE.h) }))}
          custom={customSize ?? { width: Math.round(natural?.w ?? FALLBACK_SIZE.w), height: Math.round(natural?.h ?? FALLBACK_SIZE.h) }}
          onCustomChange={setCustomSize}
          presets={[]}
        />
        <span className="ml-1 hidden truncate text-xs text-muted-foreground xl:inline">
          {basename(tab.path)}
        </span>
      </div>
      <ToolbarToggleGap />
    </>
  );

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-border bg-muted/20 px-2">
        {toolbar}
      </div>
      <PreviewCanvas
        canvas={canvas}
        renderFrame={(frame) => (
          <div
            ref={previewRef}
            style={{ width: frame.width, height: frame.height }}
            className="[&>svg]:block [&>svg]:h-full [&>svg]:w-full [&>svg]:max-h-none [&>svg]:max-w-none"
            dangerouslySetInnerHTML={{ __html: safe }}
          />
        )}
      />
    </div>
  );
}
