import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  FileText,
  Loader2,
  MessageSquare,
  Wrench,
  X,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { openFileLink } from '@/lib/file-links';
import { useExternalAgents } from '@/state/external-agents';

const EMPTY = [];

/**
 * Fold raw hook events into display rows: a tool call's start and end become
 * one row, and an approval request is "pending" only while nothing has
 * happened after it.
 */
export function foldActivity(events) {
  const items = [];
  const byCall = new Map();
  for (const ev of events) {
    if (ev.kind === 'tool') {
      const known = ev.callId ? byCall.get(ev.callId) : null;
      if (known) {
        if (ev.phase === 'end') known.status = ev.error ? 'error' : 'done';
        continue;
      }
      const item = {
        kind: 'tool',
        ts: ev.ts,
        tool: ev.tool,
        text: ev.text,
        paths: ev.paths ?? [],
        status: ev.phase === 'end' ? (ev.error ? 'error' : 'done') : 'running',
      };
      if (ev.callId) byCall.set(ev.callId, item);
      items.push(item);
    } else if (ev.kind === 'prompt' || ev.kind === 'approval' || ev.kind === 'stop') {
      items.push({ kind: ev.kind, ts: ev.ts, tool: ev.tool, text: ev.text, paths: ev.paths ?? [], error: !!ev.error });
    }
  }
  const last = events[events.length - 1];
  // A turn that ended (or a new prompt) means nothing is still running.
  let open = true;
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind === 'stop' || it.kind === 'prompt') open = false;
    if (it.kind === 'tool' && it.status === 'running' && !open) it.status = 'done';
  }
  return { items, pending: last?.kind === 'approval' ? last : null };
}

/** Last path segment, for a chip label. */
function baseName(p) {
  const parts = String(p).split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || p;
}

function ToolStatus({ status }) {
  if (status === 'running') return <Loader2 className="size-3 shrink-0 animate-spin text-info" />;
  if (status === 'error') return <X className="size-3 shrink-0 text-destructive" />;
  return <Check className="size-3 shrink-0 text-muted-foreground" />;
}

function PathChips({ paths, baseDir }) {
  if (!paths?.length) return null;
  return (
    <span className="flex shrink-0 items-center gap-1">
      {paths.map((p) => (
        <button
          key={p}
          type="button"
          title={p}
          onClick={() => openFileLink(p, { baseDir })}
          className="flex max-w-[160px] items-center gap-1 rounded border border-border bg-muted/40 px-1.5 py-px text-[11px] text-foreground/90 hover:bg-muted"
        >
          <FileText className="size-3 shrink-0 text-info" />
          <span className="truncate">{baseName(p)}</span>
        </button>
      ))}
    </span>
  );
}

function Row({ item, baseDir }) {
  if (item.kind === 'prompt') {
    return (
      <div className="flex items-start gap-2 py-1">
        <MessageSquare className="mt-0.5 size-3 shrink-0 text-info" />
        <span className="line-clamp-2 min-w-0 flex-1 text-xs text-foreground">{item.text}</span>
      </div>
    );
  }
  if (item.kind === 'stop') {
    if (!item.text && !item.error) return <div className="my-1 border-t border-border/60" />;
    return (
      <div className="flex items-start gap-2 border-b border-border/60 py-1">
        <Check className={cn('mt-0.5 size-3 shrink-0', item.error ? 'text-destructive' : 'text-muted-foreground')} />
        <span className="line-clamp-2 min-w-0 flex-1 text-xs text-muted-foreground">
          {item.text || 'Turn ended with an error'}
        </span>
      </div>
    );
  }
  if (item.kind === 'approval') {
    return (
      <div className="flex items-center gap-2 py-0.5 pl-5">
        <AlertTriangle className="size-3 shrink-0 text-warning" />
        <span className="shrink-0 text-[11px] text-muted-foreground">asked to approve</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">{item.text}</span>
      </div>
    );
  }
  // A path already shown as a chip doesn't need repeating as the summary.
  const summary = item.paths?.includes(item.text) ? null : item.text;
  return (
    <div className="flex items-center gap-2 py-0.5 pl-5">
      <ToolStatus status={item.status} />
      <span className="shrink-0 text-[11px] font-medium text-foreground/90">{item.tool || 'tool'}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground" title={summary || undefined}>
        {summary}
      </span>
      <PathChips paths={item.paths} baseDir={baseDir} />
    </div>
  );
}

/**
 * What a CLI agent session has been doing, as reported by the CLI's own hooks:
 * each prompt, each tool call (with the files it touched, clickable) and a
 * notice while the CLI waits for an approval. Read-only: nothing here is sent
 * to the CLI, and approvals are still answered in the terminal below.
 */
export function CliActivityList({ row, live }) {
  const events = useExternalAgents((s) => s.activityBySession[row.id]) ?? EMPTY;
  const [open, setOpen] = useState(false);
  const scrollRef = useRef(null);

  useEffect(() => {
    useExternalAgents.getState().loadActivity(row.id);
  }, [row.id]);

  const { items, pending } = useMemo(() => foldActivity(events), [events]);
  const waiting = live && pending;

  useEffect(() => {
    const el = scrollRef.current;
    if (open && el) el.scrollTop = el.scrollHeight;
  }, [open, items.length]);

  if (items.length === 0) return null;
  const latest = [...items].reverse().find((i) => i.kind === 'tool');
  const calls = items.filter((i) => i.kind === 'tool').length;

  return (
    <div className="shrink-0 border-b border-border bg-card/40">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex h-7 w-full items-center gap-1.5 px-2 text-left text-[11px] text-muted-foreground hover:bg-muted/40"
      >
        {open ? <ChevronDown className="size-3 shrink-0" /> : <ChevronRight className="size-3 shrink-0" />}
        <Wrench className="size-3 shrink-0 text-info" />
        <span className="shrink-0 font-medium text-foreground/90">Activity</span>
        <span className="shrink-0 tabular-nums">{calls}</span>
        {waiting ? (
          <span className="ml-1 flex min-w-0 items-center gap-1 rounded-full bg-warning/15 px-1.5 py-px text-warning">
            <AlertTriangle className="size-3 shrink-0" />
            <span className="truncate">
              {pending.tool === 'AskUserQuestion' ? 'Waiting for your answer' : 'Waiting for your approval'} below
            </span>
          </span>
        ) : (
          !open &&
          latest && (
            <span className="ml-1 min-w-0 flex-1 truncate font-mono">
              {latest.tool} {latest.paths?.includes(latest.text) ? baseName(latest.text) : latest.text}
            </span>
          )
        )}
      </button>
      {open && (
        <div ref={scrollRef} className="max-h-[38vh] overflow-y-auto px-2 pb-1.5">
          {items.map((item, i) => (
            <Row key={`${item.ts}-${i}`} item={item} baseDir={row.cwd || null} />
          ))}
        </div>
      )}
    </div>
  );
}
