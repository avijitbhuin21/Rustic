import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import {
  Brain,
  Check,
  ChevronDown,
  Copy,
  Loader2,
  Undo2,
  X,
} from 'lucide-react';
import { motion, AnimatePresence } from 'framer-motion';
import { toast } from 'sonner';
import { confirm } from '@/components/confirm-dialog';
import { useAgent } from '@/state/agent';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { ToolCallCard } from './tool-call-card';
import { AskUserInline } from './ask-user-inline';
import { cn } from '@/lib/utils';
import { useRelativeTime } from '@/lib/relative-time';

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
  const ref = useRef(null);

  // Route link clicks through Tauri's shell.open so they land in the user's
  // default browser instead of replacing the chat view. Delegated on the
  // wrapper rather than attached per-anchor because the HTML is injected via
  // dangerouslySetInnerHTML — React doesn't see the anchors. In-page anchors
  // (`#section`) keep their native behaviour.
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const onClick = (e) => {
      const anchor = e.target?.closest?.('a');
      if (!anchor) return;
      const href = anchor.getAttribute('href');
      if (!href || href.startsWith('#')) return;
      e.preventDefault();
      e.stopPropagation();
      import('@tauri-apps/plugin-shell')
        .then(({ open }) => open(href))
        .catch((err) => toast.error(`Failed to open link: ${err}`));
    };
    el.addEventListener('click', onClick);
    return () => el.removeEventListener('click', onClick);
  }, [html]);

  return (
    <div
      ref={ref}
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

// Read-only attachment chip for SENT user messages — mirrors the prompt
// box's AttachmentChip styling so a sent message visually echoes what the
// user just typed, but drops the remove (X) button and surfaces the
// image's natural dimensions next to the filename. Click anywhere on the
// chip body to open the same full-screen lightbox.
function SentAttachmentChip({ src, name }) {
  const [open, setOpen] = useState(false);
  const [dims, setDims] = useState(null);
  if (!src) return null;
  return (
    <>
      <div
        className="group relative inline-flex items-stretch overflow-hidden rounded-md border border-border/60 bg-muted/40"
        title={name || 'attachment'}
      >
        <button
          type="button"
          onClick={() => setOpen(true)}
          aria-label={`Open ${name || 'attachment'} full size`}
          className="flex cursor-zoom-in items-center gap-1.5 px-1 py-1 pr-2 text-left hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
        >
          <img
            src={src}
            alt={name || 'attachment'}
            onLoad={(e) => {
              const t = e.currentTarget;
              if (t.naturalWidth && t.naturalHeight) {
                setDims({ w: t.naturalWidth, h: t.naturalHeight });
              }
            }}
            className="size-8 shrink-0 rounded object-cover"
          />
          <span className="max-w-[140px] truncate text-[11px] text-foreground/80">
            {name || 'image'}
          </span>
          {dims && (
            <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
              {dims.w}×{dims.h}
            </span>
          )}
        </button>
      </div>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          showCloseButton={false}
          className="w-screen max-w-[100vw] gap-0 border-none bg-transparent p-0 ring-0 shadow-none sm:max-w-[100vw]"
        >
          <DialogTitle className="sr-only">Image Viewer</DialogTitle>
          <div
            className="flex h-screen w-screen cursor-zoom-out items-center justify-center p-6"
            onClick={() => setOpen(false)}
          >
            <img
              src={src}
              alt={name || 'attachment'}
              onClick={(e) => e.stopPropagation()}
              className="max-h-[92vh] max-w-[92vw] cursor-default rounded-md object-contain shadow-2xl"
            />
          </div>
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-label="Close image"
            className="fixed right-4 top-4 z-[60] flex size-10 items-center justify-center rounded-full bg-background/70 text-foreground shadow-md backdrop-blur hover:bg-background"
          >
            <X className="size-5" />
          </button>
        </DialogContent>
      </Dialog>
    </>
  );
}

function ImageAttachment({ src, alt }) {
  const [open, setOpen] = useState(false);
  if (!src) return null;
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-label={`Open ${alt || 'attachment'} full size`}
        className="my-1 block cursor-zoom-in overflow-hidden rounded-md border border-border bg-background transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <img
          src={src}
          alt={alt || 'attachment'}
          className="max-h-48 object-contain"
        />
      </button>
      {/* Full-screen viewer. DialogContent already provides its own portal +
          overlay; we strip its card chrome (bg/padding/ring/size limits) so
          the image takes the whole viewport. Radix handles overlay-click and
          Escape natively — no custom dismiss wiring needed. */}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          showCloseButton={false}
          className="w-screen max-w-[100vw] gap-0 border-none bg-transparent p-0 ring-0 shadow-none sm:max-w-[100vw]"
        >
          <DialogTitle className="sr-only">Image Viewer</DialogTitle>
          <div
            className="flex h-screen w-screen cursor-zoom-out items-center justify-center p-6"
            onClick={() => setOpen(false)}
          >
            <img
              src={src}
              alt={alt || 'attachment'}
              onClick={(e) => e.stopPropagation()}
              className="max-h-[92vh] max-w-[92vw] cursor-default rounded-md object-contain shadow-2xl"
            />
          </div>
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-label="Close image"
            className="fixed right-4 top-4 z-[60] flex size-10 items-center justify-center rounded-full bg-background/70 text-foreground shadow-md backdrop-blur hover:bg-background"
          >
            <X className="size-5" />
          </button>
        </DialogContent>
      </Dialog>
    </>
  );
}

// Collapses user messages longer than 3 lines and animates the expand toggle.
// Measures the natural height on first paint (useLayoutEffect runs before
// browser paint, so the user never sees the un-collapsed flash), then drives
// the wrapper height via framer-motion. The first render returns a plain div
// so the initial measured → collapsed transition isn't animated; subsequent
// expand / collapse interactions DO animate.
function CollapsibleUserText({ text, actions }) {
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

  // The action row at the bottom hosts both the Show more / Show less toggle
  // (when the text is collapsible) and any caller-supplied `actions` (copy +
  // revert in the user-message header). Rendered as a single flex row so the
  // toggle sits on the left and the action buttons hug the right edge.
  const showActionRow = canCollapse || !!actions;

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
      {showActionRow && (
        <div className="mt-1 flex items-center justify-between gap-2">
          {canCollapse ? (
            <button
              type="button"
              onClick={() => setExpanded((e) => !e)}
              className="inline-flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground"
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
          ) : (
            <span />
          )}
          {actions && (
            <div className="flex items-center gap-0.5">{actions}</div>
          )}
        </div>
      )}
    </>
  );
}

// Copy-to-clipboard button shown on hover over a user message. Flips to a check
// icon for ~1.2s after a successful copy so the user gets feedback without a
// toast. Falls back to a textarea+execCommand path when the Clipboard API isn't
// available (older WebView2 builds, non-secure contexts).
function CopyButton({ text }) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef(null);

  const onCopy = async () => {
    if (!text) return;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
      } else {
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        document.body.removeChild(ta);
      }
      setCopied(true);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setCopied(false), 1200);
    } catch {
      // swallow — copy is best-effort
    }
  };

  return (
    <button
      type="button"
      onClick={onCopy}
      aria-label={copied ? 'Copied' : 'Copy message'}
      title={copied ? 'Copied' : 'Copy message'}
      className="flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
    >
      {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
    </button>
  );
}

// Revert-to-this-checkpoint button shown on hover over a user message that has
// an associated file-history snapshot. Calls `fh_plan_revert_from_message` for
// a preview, surfaces a confirm dialog with the per-file plan, then on confirm
// runs `fh_revert_from_message` (restore worktree + todos) followed by
// `truncate_task_messages` (chop chat history back to before this message).
//
// Failure modes surfaced to the user:
//   - Snapshot evicted (retention dropped it): "snapshot not found" backend error
//     → toast with "snapshot too old".
//   - Per-file failures inside revert (locked file, permission denied): backend
//     returns a Failed outcome per row → toast with the first error, but the
//     chat history is still truncated (partial revert is the documented
//     behaviour of the underlying tracker).
function RevertButton({
  taskId,
  projectRoot,
  snapshotMessageId,
  userMessageIndex,
  userText: userMessageText,
  userAttachments,
}) {
  const [busy, setBusy] = useState(false);
  // Block revert while the executor is mid-turn: the persist worker would
  // race the truncation and resurrect the dropped messages, and reverting
  // disk state under a running tool would scramble the agent's view of the
  // worktree. User can hit Stop first.
  const isStreaming = useAgent((s) =>
    taskId ? !!s.streamingByTask[taskId] : false,
  );
  const setPendingDraft = useAgent((s) => s.setPendingDraft);

  const onRevert = async () => {
    if (busy || isStreaming) return;
    if (!taskId || !projectRoot || !snapshotMessageId) return;
    setBusy(true);
    try {
      let plan = [];
      try {
        plan = await invoke('fh_plan_revert_from_message', {
          projectRoot,
          messageId: snapshotMessageId,
        });
      } catch (err) {
        const msg = typeof err === 'string' ? err : err?.message || String(err);
        if (msg.toLowerCase().includes('not found')) {
          toast.error(
            'Checkpoint expired — this snapshot was dropped by retention.',
          );
        } else {
          toast.error(`Couldn't preview revert: ${msg}`);
        }
        return;
      }

      const restoreCount = plan.filter((p) => p.action === 'restore').length;
      const deleteCount = plan.filter((p) => p.action === 'delete').length;
      const totalFiles = restoreCount + deleteCount;

      const summaryParts = [];
      if (restoreCount > 0) {
        summaryParts.push(
          `${restoreCount} file${restoreCount === 1 ? '' : 's'} restored`,
        );
      }
      if (deleteCount > 0) {
        summaryParts.push(
          `${deleteCount} file${deleteCount === 1 ? '' : 's'} deleted`,
        );
      }
      const fileSummary =
        summaryParts.length > 0
          ? summaryParts.join(', ') + '.'
          : 'No files changed since this message.';

      const details =
        totalFiles > 0 ? (
          <div className="rounded border border-border/40 bg-foreground/[0.03] p-2">
            <ul className="max-h-40 space-y-0.5 overflow-auto font-mono text-[11px]">
              {plan.slice(0, 40).map((row) => (
                <li
                  key={row.path}
                  className={
                    row.action === 'delete'
                      ? 'text-rose-500'
                      : 'text-emerald-500'
                  }
                >
                  <span className="inline-block w-12 uppercase opacity-70">
                    {row.action}
                  </span>
                  {row.path}
                </li>
              ))}
              {plan.length > 40 && (
                <li className="text-muted-foreground">
                  …and {plan.length - 40} more
                </li>
              )}
            </ul>
          </div>
        ) : null;

      // Two-action confirm: primary "Chat + files" rolls back the worktree
      // AND truncates chat history (and seeds the prompt with the original
      // message so the user can re-edit and resend). Secondary "Files only"
      // just rolls the worktree back — chat stays. Cancel aborts both.
      const choice = await confirm({
        title: 'Revert to this checkpoint?',
        description: `${fileSummary}\n\n"Chat + files" also removes every message after this one (and restores it to the prompt for editing). "Files only" leaves the chat history alone.`,
        details,
        confirmLabel: 'Chat + files',
        secondaryConfirmLabel: 'Files only',
        secondaryConfirmValue: 'files-only',
        cancelLabel: 'Cancel',
        destructive: true,
      });
      if (!choice) return;
      const filesOnly = choice === 'files-only';

      try {
        const outcomes = await invoke('fh_revert_from_message', {
          projectRoot,
          messageId: snapshotMessageId,
        });
        const failed = (Array.isArray(outcomes) ? outcomes : []).filter(
          (o) => o.action === 'failed',
        );
        if (failed.length > 0) {
          toast.error(
            failed[0].error
              ? `Revert partially failed: ${failed[0].error}`
              : `Revert partially failed (${failed.length} files).`,
          );
        }
      } catch (err) {
        const msg = typeof err === 'string' ? err : err?.message || String(err);
        toast.error(`Revert failed: ${msg}`);
        return;
      }

      if (filesOnly) {
        // Refresh the task's net-changes panel since files moved. Skip
        // chat truncation entirely so the existing conversation continues
        // from where it was.
        useAgent.setState((s) => {
          if (!s.filesByTask[taskId]) return s;
          return {
            filesByTask: {
              ...s.filesByTask,
              [taskId]: { entries: [], lastMessageId: null },
            },
          };
        });
        toast.success('Files reverted. Chat history left intact.');
        return;
      }

      // Chop chat history. If we don't have a userMessageIndex (older messages,
      // hydration didn't set it), fall back to truncating the frontend list
      // based on the message id; the backend list will be brought back in
      // sync by the next persist callback.
      if (typeof userMessageIndex === 'number') {
        try {
          await invoke('truncate_task_messages', {
            taskId,
            keepCount: userMessageIndex,
          });
        } catch (err) {
          const msg =
            typeof err === 'string' ? err : err?.message || String(err);
          toast.error(`Couldn't truncate chat history: ${msg}`);
        }
      }

      // Mirror the truncation in frontend state so the UI immediately
      // re-renders without the reverted turns. Locate the user message by its
      // snapshot id (its position in the in-memory list may not match
      // userMessageIndex when condensation or other transforms have shifted
      // things).
      useAgent.setState((s) => {
        const list = s.messagesByTask[taskId];
        if (!Array.isArray(list)) return s;
        const idx = list.findIndex(
          (m) => m.snapshotMessageId === snapshotMessageId,
        );
        if (idx < 0) return s;
        return {
          messagesByTask: {
            ...s.messagesByTask,
            [taskId]: list.slice(0, idx),
          },
        };
      });

      // Seed the prompt box with the original user message + attachments so
      // the user can tweak and resend without retyping. PromptBox watches
      // `pendingDraft` and applies it once.
      setPendingDraft({
        taskId,
        text: userMessageText || '',
        attachments: Array.isArray(userAttachments) ? userAttachments : [],
      });

      toast.success('Reverted to checkpoint.');

      // Also refresh the task's net-changes panel — the file dock derives its
      // entries from filesByTask, which may be stale after revert. Cheapest
      // fix: clear it and let the next refresh repopulate.
      useAgent.setState((s) => {
        if (!s.filesByTask[taskId]) return s;
        return {
          filesByTask: {
            ...s.filesByTask,
            [taskId]: { entries: [], lastMessageId: null },
          },
        };
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <button
      type="button"
      onClick={onRevert}
      disabled={busy || isStreaming}
      aria-label={
        isStreaming
          ? 'Stop the task before reverting'
          : 'Revert to this checkpoint'
      }
      title={
        isStreaming
          ? 'Stop the task before reverting'
          : 'Revert to this checkpoint'
      }
      className="flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
    >
      {busy ? (
        <Loader2 className="size-3.5 animate-spin" />
      ) : (
        <Undo2 className="size-3.5" />
      )}
    </button>
  );
}

// The agent appends an `[Attached images]\n- path\n- path…` footer to the
// backend message so the model sees both the inline image and its on-disk path.
// That footer is purely for the model — the chat UI already renders the image
// thumbnails above the bubble, so we strip it from the rendered text.
const ATTACHED_IMAGES_FOOTER_RE =
  /(?:\n\n)?\[Attached images\](?:\n[ \t]*-[^\n]*)+\s*$/;

function stripAttachedImagesFooter(text) {
  if (!text) return text;
  return text.replace(ATTACHED_IMAGES_FOOTER_RE, '');
}

// Pull a plain-text representation out of a user message's content blocks so
// we can render it directly in the sticky header without re-using the full
// markdown renderer.
function userText(message) {
  if (!message) return '';
  const blocks = message.content || [];
  return stripAttachedImagesFooter(
    blocks
      .filter((b) => b.type === 'text')
      .map((b) => b.text || '')
      .join('\n'),
  ).trim();
}

export function ChatTurn({ turn, toolResults, taskId, projectRoot }) {
  const { user, blocks } = turn;
  const text = userText(user);
  const attachments = user?.attachments || [];
  const snapshotMessageId = user?.snapshotMessageId || null;
  const userMessageIndex = user?.userMessageIndex;
  const canRevert =
    !!snapshotMessageId && !!taskId && !!projectRoot;
  const userRelative = useRelativeTime(user?.timestamp);

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
              {attachments.length > 0 && (
                <div className={cn('flex flex-wrap gap-2', text && 'mb-2')}>
                  {attachments.map((att, idx) => (
                    <SentAttachmentChip
                      key={`att-${idx}`}
                      src={att.url || att.src}
                      name={att.name}
                    />
                  ))}
                </div>
              )}
              {text && (
                <CollapsibleUserText
                  text={text}
                  actions={
                    text || canRevert ? (
                      <>
                        {userRelative && (
                          <span
                            title={
                              user?.timestamp
                                ? new Date(user.timestamp).toLocaleString()
                                : undefined
                            }
                            className="select-none text-[10px] tabular-nums text-muted-foreground"
                          >
                            {userRelative}
                          </span>
                        )}
                        {canRevert && (
                          <RevertButton
                            taskId={taskId}
                            projectRoot={projectRoot}
                            snapshotMessageId={snapshotMessageId}
                            userMessageIndex={userMessageIndex}
                            userText={text}
                            userAttachments={attachments}
                          />
                        )}
                        {text && <CopyButton text={text} />}
                      </>
                    ) : null
                  }
                />
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
                      timestamp={blocks[idx]?.timestamp}
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
                if (block.type === 'ask_user') {
                  return (
                    <AskUserInline
                      key={`${messageId}-${idx}`}
                      requestId={block.request_id}
                      questions={block.questions}
                      answered={!!block.answered}
                      answers={block.answers}
                      cancelled={!!block.cancelled}
                    />
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
