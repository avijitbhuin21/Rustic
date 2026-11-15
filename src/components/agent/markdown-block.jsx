import React, { useEffect, useMemo, useRef } from 'react';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';
import { useCodeCopyButtons } from '@/lib/code-copy';
import { handleMarkdownLinkClick } from '@/lib/markdown-assets';
import { activeChatRoot, handleFileLinkClick, linkifyFilePaths } from '@/lib/file-links';
import { useMermaidBlocks } from '@/lib/mermaid';

// Bounded LRU-ish cache of rendered markdown, keyed by raw source text.
// The virtualizer unmounts off-screen turns, so scrolling back up otherwise
// re-runs marked.parse + DOMPurify.sanitize + linkifyFilePaths on identical
// text every time a settled block re-mounts. Caching the pure (text -> html)
// result makes those remounts free. Insertion-order eviction keeps memory
// bounded; the active streaming block still re-parses each flush (its text
// changes), which is expected.
const MARKDOWN_CACHE = new Map();
const MARKDOWN_CACHE_MAX = 300;

function renderMarkdown(text) {
  if (!text) return '';
  const cached = MARKDOWN_CACHE.get(text);
  if (cached !== undefined) return cached;
  let html;
  try {
    html = linkifyFilePaths(DOMPurify.sanitize(marked.parse(text, { breaks: true, gfm: true })));
  } catch {
    html = DOMPurify.sanitize(text);
  }
  if (MARKDOWN_CACHE.size >= MARKDOWN_CACHE_MAX) {
    const oldest = MARKDOWN_CACHE.keys().next().value;
    if (oldest !== undefined) MARKDOWN_CACHE.delete(oldest);
  }
  MARKDOWN_CACHE.set(text, html);
  return html;
}

export function MarkdownBlock({ text, className }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  const ref = useRef(null);
  // Subscribe so the handler re-binds when the viewed task/project changes.
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const activeRoot = useAgent((s) => s.activeProject?.root || null);

  // Route link clicks through the shared markdown link handler (scheme
  // allow-listed shell.open for external URLs, editor tab for local paths).
  // Delegated on the wrapper rather than attached per-anchor because the HTML
  // is injected via dangerouslySetInnerHTML — React doesn't see the anchors.
  // In-page anchors (`#section`) keep their native behaviour. File-path spans
  // tagged by linkifyFilePaths open the cited file (and line) in the editor,
  // resolved against the project that owns the chat's task (not whichever
  // project happens to be picked in the agent panel).
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const onClick = (e) => {
      const root = activeChatRoot();
      if (handleFileLinkClick(e, root)) return;
      handleMarkdownLinkClick(e, root);
    };
    el.addEventListener('click', onClick);
    return () => el.removeEventListener('click', onClick);
  }, [html, activeTaskId, activeRoot]);

  // Add a hover copy button to each fenced code block in this message.
  useCodeCopyButtons(ref, [html]);
  // Render ```mermaid fences as inline diagrams once they parse cleanly.
  useMermaidBlocks(ref, [html]);

  return (
    <div
      ref={ref}
      data-agent-message
      className={cn(
        'prose-chat text-[length:var(--chat-fs,0.75rem)] leading-relaxed [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[length:var(--chat-code-fs,11px)] [&_p]:my-1 [&_pre]:my-2 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted/70 [&_pre]:p-2 [&_pre]:text-[length:var(--chat-code-fs,11px)] [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_ul]:my-1 [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:my-1 [&_ol]:list-decimal [&_ol]:pl-5 [&_h1]:my-2 [&_h1]:text-sm [&_h1]:font-semibold [&_h2]:my-2 [&_h2]:text-xs [&_h2]:font-semibold [&_h3]:my-2 [&_h3]:text-xs [&_h3]:font-semibold',
        className,
      )}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
