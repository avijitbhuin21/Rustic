import React, { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { Brain, ChevronDown, Loader2 } from 'lucide-react';
import { motion, AnimatePresence } from 'framer-motion';
import { ToolCallCard } from './tool-call-card';

function renderMarkdown(text) {
  if (!text) return '';
  try {
    return DOMPurify.sanitize(marked.parse(text, { breaks: true, gfm: true }));
  } catch {
    return DOMPurify.sanitize(text);
  }
}

function MarkdownBlock({ text }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  return (
    <div
      data-agent-message
      className="prose-chat text-xs leading-relaxed [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[11px] [&_p]:my-1 [&_pre]:my-2 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted/70 [&_pre]:p-2 [&_pre]:text-[11px] [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_ul]:my-1 [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:my-1 [&_ol]:list-decimal [&_ol]:pl-5 [&_h1]:my-2 [&_h1]:text-sm [&_h1]:font-semibold [&_h2]:my-2 [&_h2]:text-xs [&_h2]:font-semibold [&_h3]:my-2 [&_h3]:text-xs [&_h3]:font-semibold"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

const panelVariants = {
  hidden: { opacity: 0, height: 0 },
  visible: {
    opacity: 1,
    height: 'auto',
    transition: { duration: 0.25, ease: [0.2, 0.65, 0.3, 0.9] },
  },
  exit: {
    opacity: 0,
    height: 0,
    transition: { duration: 0.2, ease: [0.2, 0.65, 0.3, 0.9] },
  },
};

// Borderless row for an extended-thinking block. Auto-opens while streaming so
// the user can watch the thought form, collapses to a one-line "Reasoned for
// Ns" once `done` flips true. Sits visually on the turn's dashed connecting
// line via a punch-through bg on the icon wrapper.
function ThinkingRow({ text, done, durationSecs }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex flex-col">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="group flex w-full items-center gap-2 rounded-md py-1 pr-2 text-left text-xs hover:bg-foreground/[0.04]"
      >
        <span className="relative z-10 flex shrink-0 bg-sidebar">
          {/* Inner overlay matches the row's group-hover tint so the icon
              blends with the rest of the row on hover. Outer stays opaque
              (bg-sidebar) so the dashed turn-rail stays hidden behind it. */}
          <span className="flex items-center justify-center px-0.5 group-hover:bg-foreground/[0.04]">
            {done ? (
              <Brain className="size-4 text-muted-foreground" />
            ) : (
              <Loader2 className="size-4 animate-spin text-blue-500" />
            )}
          </span>
        </span>
        <span className="min-w-0 flex-1 truncate font-medium text-muted-foreground">
          {done ? `Reasoned for ${durationSecs ?? 0}s` : 'Thinking…'}
        </span>
      </button>
      <AnimatePresence initial={false}>
        {open && text && (
          <motion.div
            variants={panelVariants}
            initial="hidden"
            animate="visible"
            exit="exit"
            className="overflow-hidden"
          >
            <div className="ml-2 mt-1 mb-1 pl-5">
              <pre className="whitespace-pre-wrap break-words font-sans text-[12px] italic leading-relaxed text-muted-foreground">
                {text}
              </pre>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function ImageAttachment({ src, alt }) {
  return (
    <img
      src={src}
      alt={alt || 'attachment'}
      className="my-1 max-h-48 rounded-md border border-border object-contain"
    />
  );
}

// Collapses user messages longer than 3 lines and animates the expand toggle.
// Measures the natural height on first paint (useLayoutEffect runs before
// browser paint, so the user never sees the un-collapsed flash), then drives
// the wrapper height via framer-motion. The first render returns a plain div
// so the initial measured → collapsed transition isn't animated; subsequent
// expand / collapse interactions DO animate.
function CollapsibleUserText({ text }) {
  const [expanded, setExpanded] = useState(false);
  const innerRef = useRef(null);
  const [heights, setHeights] = useState(null);

  useLayoutEffect(() => {
    const inner = innerRef.current;
    if (!inner) return;
    const full = inner.scrollHeight;
    const styles = window.getComputedStyle(inner);
    const lh = parseFloat(styles.lineHeight) || 20;
    const collapsed = Math.ceil(lh * 3);
    setHeights({
      full,
      collapsed,
      canCollapse: full > collapsed + 2,
    });
  }, [text]);

  const canCollapse = heights?.canCollapse ?? false;
  const showCollapsed = canCollapse && !expanded;

  if (!heights) {
    return (
      <div
        ref={innerRef}
        className="whitespace-pre-wrap text-xs leading-relaxed text-foreground"
      >
        {text}
      </div>
    );
  }

  return (
    <>
      <motion.div
        initial={false}
        animate={{ height: showCollapsed ? heights.collapsed : heights.full }}
        transition={{ duration: 0.25, ease: [0.2, 0.65, 0.3, 0.9] }}
        style={{ overflow: 'hidden' }}
      >
        <div
          ref={innerRef}
          className="whitespace-pre-wrap text-xs leading-relaxed text-foreground"
        >
          {text}
        </div>
      </motion.div>
      {canCollapse && (
        <button
          type="button"
          onClick={() => setExpanded((e) => !e)}
          className="mt-1 inline-flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground"
        >
          <motion.span
            animate={{ rotate: expanded ? 180 : 0 }}
            transition={{ duration: 0.2 }}
            className="inline-flex"
          >
            <ChevronDown className="size-3" />
          </motion.span>
          {expanded ? 'Show less' : 'Show more'}
        </button>
      )}
    </>
  );
}

// Pull a plain-text representation out of a user message's content blocks so
// we can render it directly in the sticky header without re-using the full
// markdown renderer.
function userText(message) {
  if (!message) return '';
  const blocks = message.content || [];
  return blocks
    .filter((b) => b.type === 'text')
    .map((b) => b.text || '')
    .join('\n')
    .trim();
}

export function ChatTurn({ turn, toolResults }) {
  const { user, blocks } = turn;
  const text = userText(user);
  const attachments = user?.attachments || [];

  return (
    <div className="flex flex-col">
      {user && (
        // Sticky user message. As the user scrolls down through the assistant
        // output for this turn, this header pins to the top of the scroll
        // viewport. When the next turn comes into view, its own sticky header
        // pushes this one out.
        <div className="sticky top-0 z-20">
          <div className="mx-auto w-full max-w-3xl px-3 pt-2">
            <div className="rounded-md border border-border/50 bg-muted/60 px-3 py-2 backdrop-blur-sm">
              {text && <CollapsibleUserText text={text} />}
              {attachments.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-2">
                  {attachments.map((att, idx) => (
                    <ImageAttachment
                      key={`att-${idx}`}
                      src={att.url || att.src}
                      alt={att.name}
                    />
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {blocks.length > 0 && (
        <div className="mx-auto w-full max-w-3xl px-3 py-3">
          {/*
            Plan-style container: a continuous vertical dashed line connects
            every assistant block (thinking, text, tool_use) for this turn.
            Block rows position their status icon on the line and use a
            background-colored wrapper to "punch through" it, mimicking the
            agent-plan reference. Text blocks have no icon and just sit
            indented in the column to the right of the line.
          */}
          <div className="relative">
            <div className="pointer-events-none absolute bottom-2 left-[9px] top-2 border-l-2 border-dashed border-muted-foreground/25" />
            <div className="space-y-1">
              {blocks.map(({ block, messageId, streaming }, idx) => {
                if (block.type === 'text') {
                  return (
                    <div key={`${messageId}-${idx}`} className="relative py-1 pl-7">
                      <MarkdownBlock text={block.text} />
                      {streaming && idx === blocks.length - 1 && (
                        <span className="ml-1 inline-block size-1.5 animate-pulse rounded-full bg-foreground/60 align-middle" />
                      )}
                    </div>
                  );
                }
                if (block.type === 'thinking') {
                  return (
                    <ThinkingRow
                      key={`${messageId}-${idx}`}
                      text={block.text}
                      done={!!block.done}
                      durationSecs={block.durationSecs}
                    />
                  );
                }
                if (block.type === 'tool_use') {
                  const result = toolResults?.[block.id];
                  return (
                    <ToolCallCard
                      key={`${messageId}-${idx}`}
                      name={block.name}
                      input={block.input}
                      output={result?.output}
                      isError={result?.is_error}
                    />
                  );
                }
                if (block.type === 'image') {
                  return (
                    <div key={`${messageId}-${idx}`} className="pl-7">
                      <ImageAttachment
                        src={block.source?.url || block.url}
                      />
                    </div>
                  );
                }
                return null;
              })}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default ChatTurn;
