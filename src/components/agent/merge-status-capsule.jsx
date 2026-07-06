import React, { useState } from 'react';
import { useAgent } from '@/state/agent';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogTrigger,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';

// MergeStatusCapsule — a pill next to the context-usage capsule that appears
// only while the active task's isolated worktree is doing something merge-
// related (queued / merging / parked on conflict). Clicking it opens a modal
// with the full status + any error, so a stuck or parked merge is never
// invisible.

const STATES = {
  queued: {
    label: 'queued',
    cls: 'border-violet-500/50 text-violet-500',
    dot: 'bg-violet-500',
    title: 'Merge queued',
    desc: 'This task\u2019s changes are waiting in the merge queue. Merges for a repository run one at a time.',
  },
  merging: {
    label: 'merging',
    cls: 'border-violet-500/50 text-violet-500',
    dot: 'bg-violet-500 animate-pulse',
    title: 'Merging\u2026',
    desc: 'The merge worker is rebasing this task\u2019s changes onto the base branch and landing them.',
  },
  'needs-reconciliation': {
    label: 'conflict',
    cls: 'border-rose-500/50 text-rose-500',
    dot: 'bg-rose-500',
    title: 'Merge parked \u2014 conflict',
    desc: 'The rebase hit conflicts. The agent is asked to resolve them automatically; if it can\u2019t, resolve from the bar above the chat.',
  },
};

function fmtWhen(iso) {
  if (!iso) return null;
  const t = new Date(iso).getTime();
  if (!Number.isFinite(t)) return null;
  const secs = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.round(secs / 60)}m ago`;
  return `${Math.round(secs / 3600)}h ago`;
}

export function MergeStatusCapsule({ className }) {
  const [open, setOpen] = useState(false);
  const wt = useAgent((s) =>
    s.activeTaskId ? s.worktreeByTask[s.activeTaskId] : null,
  );

  const meta = wt ? STATES[wt.state] : null;
  if (!meta) return null;

  const queuedWhen = fmtWhen(wt.queued_at);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <button
          type="button"
          title={`Merge status: ${meta.label} — click for details`}
          className={cn(
            'flex h-[24px] select-none items-center gap-1.5 rounded-full border bg-background px-2 text-[10px] font-medium',
            meta.cls,
            className,
          )}
        >
          <span className={cn('size-1.5 rounded-full', meta.dot)} />
          {meta.label}
        </button>
      </DialogTrigger>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{meta.title}</DialogTitle>
          <DialogDescription>{meta.desc}</DialogDescription>
        </DialogHeader>
        <div className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-xs">
          <span className="text-muted-foreground">State</span>
          <span className="font-mono">{wt.state}</span>
          <span className="text-muted-foreground">Merges into</span>
          <span className="font-mono">{wt.base_branch || 'main'}</span>
          {queuedWhen && (
            <>
              <span className="text-muted-foreground">Queued</span>
              <span>{queuedWhen}</span>
            </>
          )}
          {wt.merged_oid && (
            <>
              <span className="text-muted-foreground">Last landed</span>
              <span className="font-mono">{String(wt.merged_oid).slice(0, 10)}</span>
            </>
          )}
        </div>
        {wt.last_error && (
          <div className="mt-1">
            <div className="mb-1 text-xs text-muted-foreground">Error</div>
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-md border border-border bg-muted/30 p-2 text-[11px] leading-snug">
              {wt.last_error}
            </pre>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

export default MergeStatusCapsule;
