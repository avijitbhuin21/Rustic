import React, { memo, useEffect, useMemo, useRef } from 'react';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';
import { useCodeCopyButtons } from '@/lib/code-copy';
import { handleMarkdownLinkClick } from '@/lib/markdown-assets';
import { activeChatRoot, handleFileLinkClick } from '@/lib/file-links';
import { useMermaidBlocks } from '@/lib/mermaid';
import { renderChatMarkdown } from '@/lib/chat-markdown';

// One top-level markdown block. Memoised on its HTML, so while a reply streams
// only the block that is still growing is re-injected: the settled ones keep
// their DOM, and with it their copy buttons and rendered diagrams.
// `display: contents` keeps the wrapper out of layout, so blocks space and
// collapse margins exactly as siblings of one container.
const MarkdownChunk = memo(function MarkdownChunk({ html }) {
  return <div className="contents" dangerouslySetInnerHTML={{ __html: html }} />;
});

export function MarkdownBlock({ text, className }) {
  const chunks = useMemo(() => renderChatMarkdown(text), [text]);
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
  }, [activeTaskId, activeRoot]);

  // Add a hover copy button to each fenced code block in this message.
  useCodeCopyButtons(ref, [chunks]);
  // Render ```mermaid fences as inline diagrams once they parse cleanly.
  useMermaidBlocks(ref, [chunks], { agent: true });

  return (
    <div
      ref={ref}
      data-agent-message
      className={cn(
        'prose-chat text-[length:var(--chat-fs,0.75rem)] leading-relaxed [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[length:var(--chat-code-fs,11px)] [&_p]:my-1 [&_pre]:my-2 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted/70 [&_pre]:p-2 [&_pre]:text-[length:var(--chat-code-fs,11px)] [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_ul]:my-1 [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:my-1 [&_ol]:list-decimal [&_ol]:pl-5 [&_h1]:my-2 [&_h1]:text-sm [&_h1]:font-semibold [&_h2]:my-2 [&_h2]:text-xs [&_h2]:font-semibold [&_h3]:my-2 [&_h3]:text-xs [&_h3]:font-semibold',
        className,
      )}
    >
      {chunks.map((html, i) => (
        // Position is the identity: blocks are only ever appended or re-split in place.
        <MarkdownChunk key={i} html={html} />
      ))}
    </div>
  );
}
