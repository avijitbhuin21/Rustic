import React, { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import DOMPurify from 'dompurify';
import { toast } from 'sonner';
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
  }, [tab.path]);

  const safe = useMemo(
    () =>
      DOMPurify.sanitize(text ?? '', { USE_PROFILES: { svg: true, svgFilters: true } }),
    [text],
  );

  // Intercept link clicks in the SVG preview and open them in the
  // external browser (SVG can have <a> elements with xlink:href or href).
  useEffect(() => {
    const handleClick = (e) => {
      // Check for both HTML <a> and SVG <a> elements
      const anchor = e.target.closest('a');
      if (!anchor) return;

      // SVG links can use href or xlink:href
      const href = anchor.getAttribute('href') || anchor.getAttributeNS('http://www.w3.org/1999/xlink', 'href');
      if (!href) return;

      // Allow internal anchor links (same-page navigation)
      if (href.startsWith('#')) return;

      e.preventDefault();
      e.stopPropagation();

      // Open external URLs in the default browser
      openUrl(href).catch((err) => {
        toast.error(`Failed to open link: ${err}`);
      });
    };

    const el = previewRef.current;
    if (el) {
      el.addEventListener('click', handleClick);
      return () => el.removeEventListener('click', handleClick);
    }
  }, [safe]);

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
