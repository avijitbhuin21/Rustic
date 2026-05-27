import React, { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import DOMPurify from 'dompurify';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Eye, Pencil } from 'lucide-react';
import { PreviewSurface } from './preview-surface';
import { SourceCodeEditor } from './source-code-editor';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';

export default function SvgPreview({ tab }) {
  const [text, setText] = useState(null);
  const [draft, setDraft] = useState(null);
  const [error, setError] = useState(null);
  const [mode, setMode] = useState('preview');
  const [scale, setScale] = useState(1);
  const [saving, setSaving] = useState(false);
  const tabSetDirty = useEditor((s) => s.setDirty);
  const previewRef = useRef(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setText(null);
    setDraft(null);
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (cancelled) return;
        const body = c ?? '';
        setText(body);
        setDraft(body);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  const dirty = draft !== null && draft !== text;
  const safe = useMemo(
    () =>
      DOMPurify.sanitize(draft ?? '', { USE_PROFILES: { svg: true, svgFilters: true } }),
    [draft],
  );

  const onSave = async () => {
    if (!dirty || saving) return;
    setSaving(true);
    try {
      await writeTextFile(tab.path, draft ?? '');
      setText(draft);
      toast.success('Saved');
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Save failed: ${msg}`);
    } finally {
      setSaving(false);
    }
  };

  // Mirror dirty into the tab's yellow dot + bind Ctrl+S to onSave.
  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, tab.id]);
  useEffect(() => {
    setActiveSaver(onSave);
    return () => clearActiveSaver(onSave);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.path, dirty, draft]);

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
    if (el && mode === 'preview') {
      el.addEventListener('click', handleClick);
      return () => el.removeEventListener('click', handleClick);
    }
  }, [mode]);

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

  // Unsaved state lives on the tab as a yellow dot; Ctrl+S saves.
  const toolbar = (
    <div className="flex items-center gap-1">
      <Button
        size="xs"
        variant={mode === 'preview' ? 'secondary' : 'ghost'}
        onClick={() => setMode('preview')}
      >
        <Eye className="mr-1 size-3" /> Preview
      </Button>
      <Button
        size="xs"
        variant={mode === 'edit' ? 'secondary' : 'ghost'}
        onClick={() => setMode('edit')}
      >
        <Pencil className="mr-1 size-3" /> Edit
      </Button>
    </div>
  );

  if (mode === 'edit') {
    return (
      <div className="flex h-full w-full flex-col">
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-border bg-muted/20 px-2">
          {toolbar}
        </div>
        <div className="min-h-0 flex-1">
          <SourceCodeEditor
            value={draft ?? ''}
            onChange={setDraft}
            onSave={onSave}
            lang="svg"
          />
        </div>
      </div>
    );
  }

  return (
    <PreviewSurface
      toolbar={toolbar}
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
