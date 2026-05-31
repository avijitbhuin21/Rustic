import React, { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import hljs from 'highlight.js/lib/common';
import DOMPurify from 'dompurify';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Eye, Pencil } from 'lucide-react';
import { ScrollArea } from '@/components/ui/scroll-area';
import { cn } from '@/lib/utils';
import { SourceCodeEditor } from './source-code-editor';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';
import { useCodeCopyButtons } from '@/lib/code-copy';
import 'highlight.js/styles/github-dark.css';

// Single configured marked instance — created once at module load so we
// don't rebuild the lexer + register hooks on every keystroke. The
// markedHighlight extension routes every fenced code block through
// highlight.js. We register only the "common" subset to keep the bundle
// reasonable; that gives us javascript, typescript, python, rust, json,
// bash, markdown, html/css and ~30 others.
const md = new Marked(
  markedHighlight({
    emptyLangClass: 'hljs',
    langPrefix: 'hljs language-',
    highlight(code, lang) {
      const language = hljs.getLanguage(lang) ? lang : 'plaintext';
      try {
        return hljs.highlight(code, { language, ignoreIllegals: true }).value;
      } catch {
        return code;
      }
    },
  }),
  { gfm: true, breaks: true },
);

function render(text) {
  if (!text) return '';
  const raw = md.parse(text);
  // Allow GFM checkbox inputs (DOMPurify's default profile strips <input>).
  // highlight.js emits <span class="hljs-...">; we whitelist the class
  // attribute on those spans (DOMPurify keeps class by default for known
  // tags, but explicit is safer).
  return DOMPurify.sanitize(raw, {
    ADD_TAGS: ['input'],
    ADD_ATTR: ['type', 'checked', 'disabled', 'class'],
  });
}

export default function MarkdownPreview({ tab }) {
  const [text, setText] = useState(null);
  const [draft, setDraft] = useState(null);
  const [error, setError] = useState(null);
  const [mode, setMode] = useState('preview');
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

  // Render off the draft when editing so the user sees their in-flight
  // changes the moment they flip back to Preview — without this the preview
  // would still show the last-saved content.
  const renderedHtml = useMemo(() => render(draft ?? ''), [draft]);
  const dirty = draft !== null && draft !== text;

  // Drop a hover copy button onto every fenced code block. Re-runs when the
  // rendered HTML changes or we flip back into preview mode (the preview div
  // only exists while mode === 'preview').
  useCodeCopyButtons(previewRef, [renderedHtml, mode]);

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

  // Drive the tab's yellow dot from our `dirty` derivation, and register
  // Ctrl+S so the global save command routes through us when this
  // preview is active. Pattern mirrors xlsx-preview.
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

  // Intercept link clicks in the markdown preview and open them in the
  // external browser instead of navigating the WebView.
  useEffect(() => {
    const handleClick = (e) => {
      const anchor = e.target.closest('a');
      if (!anchor) return;
      const href = anchor.getAttribute('href');
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
      <div className="flex h-full w-full flex-col gap-2 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      {/* The Preview/Edit toggle is the only thing left in this strip —
          unsaved state is signalled by the yellow dot on the file tab,
          and Ctrl+S triggers save via setActiveSaver above. */}
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-muted/20 px-2">
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
      <div className="relative min-h-0 flex-1">
        {mode === 'preview' ? (
          <ScrollArea className="h-full w-full">
            <div
              ref={previewRef}
              className={cn('rustic-markdown mx-auto max-w-3xl p-6')}
              dangerouslySetInnerHTML={{ __html: renderedHtml }}
            />
          </ScrollArea>
        ) : (
          <SourceCodeEditor
            value={draft ?? ''}
            onChange={setDraft}
            onSave={onSave}
            lang="markdown"
          />
        )}
      </div>
    </div>
  );
}
