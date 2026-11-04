import React, { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { dirname, handleMarkdownLinkClick } from '@/lib/markdown-assets';
import { renderMarkdown as render } from '@/lib/markdown-render';
import { Skeleton } from '@/components/ui/skeleton';
import { ScrollArea } from '@/components/ui/scroll-area';
import { cn } from '@/lib/utils';
import { useCodeCopyButtons } from '@/lib/code-copy';
import { ZoomControls, ToolbarToggleGap, useCtrlWheelZoom } from './preview-zoom';

const MIN_SCALE = 0.5;
const MAX_SCALE = 4;

// Pure rendered preview. Editing is handled by the Monaco editor via the
// shared Edit ⇄ Preview toggle in editor-pane.jsx (ViewModeToggle) — this
// component used to carry its OWN Preview/Edit toolbar + SourceCodeEditor,
// which duplicated that control. It now renders the file content only.
export default function MarkdownPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);
  const [scale, setScale] = useState(1);
  const previewRef = useRef(null);
  const rootRef = useRef(null);

  // Prose reflows, so this is a browser-style zoom (CSS `zoom`) rather than
  // the fit-and-magnify model the html/svg/image previews use: text rewraps
  // to the pane at every level instead of shrinking to fit it.
  useCtrlWheelZoom(rootRef, {
    scale,
    onScaleChange: setScale,
    minScale: MIN_SCALE,
    maxScale: MAX_SCALE,
  });

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

  const renderedHtml = useMemo(() => render(text ?? ''), [text]);

  // Drop a hover copy button onto every fenced code block.
  useCodeCopyButtons(previewRef, [renderedHtml]);

  // Intercept link clicks in the markdown preview via the shared handler:
  // external URLs (scheme allow-listed) open in the default browser, local
  // paths relative to this file open in an editor tab.
  useEffect(() => {
    const baseDir = dirname(tab.path);
    const handleClick = (e) => handleMarkdownLinkClick(e, baseDir);

    const el = previewRef.current;
    if (el) {
      el.addEventListener('click', handleClick);
      return () => el.removeEventListener('click', handleClick);
    }
  }, [renderedHtml, tab.path]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (text == null) {
    return (
      <div className="flex h-full w-full flex-col gap-2 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }

  return (
    <div ref={rootRef} className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-border bg-muted/20 px-2">
        <ZoomControls
          scale={scale}
          onScaleChange={setScale}
          minScale={MIN_SCALE}
          maxScale={MAX_SCALE}
          onFit={() => setScale(1)}
          fitLabel="Reset zoom"
        />
        <ToolbarToggleGap />
      </div>
      <div className="relative min-h-0 flex-1">
        <ScrollArea className="h-full w-full">
          <div
            ref={previewRef}
            className={cn('rustic-markdown mx-auto max-w-3xl p-6')}
            style={{ zoom: scale }}
            dangerouslySetInnerHTML={{ __html: renderedHtml }}
          />
        </ScrollArea>
      </div>
    </div>
  );
}
