import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { renderMermaidDiagram } from '@/lib/mermaid';
import { Skeleton } from '@/components/ui/skeleton';
import { basename } from '@/state/editor';
import { ZoomControls } from './preview-zoom';
import {
  CANVAS_MAX_SCALE,
  CANVAS_MIN_SCALE,
  PreviewCanvas,
  useCanvasView,
} from './preview-canvas';

const FALLBACK_SIZE = { w: 800, h: 600 };

/** Tracks the app's light/dark mode so the diagram re-renders on a theme switch. */
function useIsDark() {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains('dark'));
  useEffect(() => {
    const root = document.documentElement;
    const observer = new MutationObserver(() => setDark(root.classList.contains('dark')));
    observer.observe(root, { attributes: true, attributeFilter: ['class'] });
    return () => observer.disconnect();
  }, []);
  return dark;
}

/** Pannable / zoomable canvas holding one rendered diagram. */
function DiagramCanvas({ diagram, title }) {
  const frames = useMemo(
    () => [
      {
        id: 'diagram',
        label: 'Diagram',
        width: Math.max(1, Math.ceil(diagram.width || FALLBACK_SIZE.w)),
        height: Math.max(1, Math.ceil(diagram.height || FALLBACK_SIZE.h)),
      },
    ],
    [diagram.width, diagram.height],
  );
  const canvas = useCanvasView(frames);
  const hostRef = useRef(null);

  // Mermaid pins its SVG to an inline max-width; the artboard is already
  // sized to the viewBox, so let the drawing fill it and scale with the zoom.
  useLayoutEffect(() => {
    const svg = hostRef.current?.querySelector('svg');
    if (!svg) return;
    svg.style.maxWidth = 'none';
    svg.style.width = '100%';
    svg.style.height = '100%';
  }, [diagram.svg]);

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-2">
        <ZoomControls
          scale={canvas.view.scale}
          onScaleChange={canvas.setScale}
          minScale={CANVAS_MIN_SCALE}
          maxScale={CANVAS_MAX_SCALE}
          onFit={canvas.fitNow}
          fitLabel="Zoom to fit"
        />
        <span className="ml-1 hidden truncate text-xs text-muted-foreground xl:inline">{title}</span>
      </div>
      <PreviewCanvas
        canvas={canvas}
        renderFrame={() => (
          <div
            ref={hostRef}
            className="h-full w-full bg-background [&>svg]:block"
            dangerouslySetInnerHTML={{ __html: diagram.svg }}
          />
        )}
      />
    </div>
  );
}

// Rendered preview for standalone mermaid files (.mmd / .mermaid). Editing is
// handled by the Monaco editor via the shared Edit ⇄ Preview toggle in
// editor-pane.jsx.
export default function MermaidPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);
  const [diagram, setDiagram] = useState(null);
  const [renderError, setRenderError] = useState(null);
  const dark = useIsDark();

  const reloadVersion = useFileReloadVersion(tab.path);

  useEffect(() => {
    setText(null);
    setDiagram(null);
    setRenderError(null);
    setError(null);
  }, [tab.path]);

  // Unlike the path change above, a reload keeps the current diagram on
  // screen until the new one is ready, so the viewport doesn't reset on every
  // save.
  useEffect(() => {
    let cancelled = false;
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (cancelled) return;
        setError(null);
        setText(c ?? '');
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path, reloadVersion]);

  useEffect(() => {
    if (text == null || !text.trim()) return undefined;
    let cancelled = false;
    renderMermaidDiagram(text)
      .then((d) => {
        if (cancelled) return;
        setDiagram(d);
        setRenderError(null);
      })
      .catch((e) => {
        if (!cancelled) setRenderError(e?.message || String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [text, dark]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }
  if (text != null && !text.trim()) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-muted-foreground">
        This diagram file is empty.
      </div>
    );
  }
  if (renderError) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-6 text-sm">
        <p className="text-destructive">This diagram could not be rendered.</p>
        <pre className="max-h-64 max-w-full overflow-auto rounded-md bg-muted p-3 text-xs text-muted-foreground">
          {renderError}
        </pre>
        <p className="text-xs text-muted-foreground">Switch to Edit to fix the source.</p>
      </div>
    );
  }
  if (!diagram) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-64 w-64" />
      </div>
    );
  }

  return <DiagramCanvas diagram={diagram} title={basename(tab.path)} />;
}
