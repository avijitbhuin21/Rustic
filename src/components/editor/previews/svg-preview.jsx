import React, { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { dirname, handleMarkdownLinkClick } from '@/lib/markdown-assets';
import DOMPurify from 'dompurify';
import { Skeleton } from '@/components/ui/skeleton';
import { PreviewSurface } from './preview-surface';

// Pure rendered preview. Editing is handled by the Monaco editor via the
// shared Edit ⇄ Preview toggle in editor-pane.jsx — this component used to
// carry its own Preview/Edit toolbar + SourceCodeEditor, which duplicated
// that control. It now renders the SVG only (Ctrl+wheel zoom preserved).
export default function SvgPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);
  const [scale, setScale] = useState(1);
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

  return (
    <PreviewSurface
      scale={scale}
      onScaleChange={setScale}
      minScale={0.1}
      maxScale={16}
    >
      <div className="flex min-h-full w-full items-center justify-center p-4">
        <div
          ref={previewRef}
          // Scale the rendered SVG via CSS transform so Ctrl+wheel zoom (via
          // PreviewSurface) actually scales the artwork instead of just the
          // container. transform-origin: center to keep the artwork centered
          // through zoom changes.
          style={{ transform: `scale(${scale})`, transformOrigin: 'center center' }}
          className="inline-block [&>svg]:block [&>svg]:max-h-none [&>svg]:max-w-none"
          dangerouslySetInnerHTML={{ __html: safe }}
        />
      </div>
    </PreviewSurface>
  );
}
