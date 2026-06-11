import React, { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import hljs from 'highlight.js/lib/common';
import DOMPurify from 'dompurify';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { ScrollArea } from '@/components/ui/scroll-area';
import { cn } from '@/lib/utils';
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

// Pure rendered preview. Editing is handled by the Monaco editor via the
// shared Edit ⇄ Preview toggle in editor-pane.jsx (ViewModeToggle) — this
// component used to carry its OWN Preview/Edit toolbar + SourceCodeEditor,
// which duplicated that control. It now renders the file content only.
export default function MarkdownPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);
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

  const renderedHtml = useMemo(() => render(text ?? ''), [text]);

  // Drop a hover copy button onto every fenced code block.
  useCodeCopyButtons(previewRef, [renderedHtml]);

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
    if (el) {
      el.addEventListener('click', handleClick);
      return () => el.removeEventListener('click', handleClick);
    }
  }, [renderedHtml]);

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
      <div className="relative min-h-0 flex-1">
        <ScrollArea className="h-full w-full">
          <div
            ref={previewRef}
            className={cn('rustic-markdown mx-auto max-w-3xl p-6')}
            dangerouslySetInnerHTML={{ __html: renderedHtml }}
          />
        </ScrollArea>
      </div>
    </div>
  );
}
