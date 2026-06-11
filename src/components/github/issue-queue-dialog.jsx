import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  CheckCircle2, CircleDashed, Loader2, MessageCircleQuestion, OctagonX, UserRound,
} from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useLayout } from '@/state/layout';
import {
  Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { Badge } from '@/components/ui/badge';

// Visual treatment per github_issues.status (see migration 017 for the
// lifecycle). Keys are the raw DB values.
const STATUS_META = {
  queued: { label: 'Queued', icon: CircleDashed, className: 'text-muted-foreground' },
  working: { label: 'Working', icon: Loader2, className: 'text-blue-500', spin: true },
  waiting_reply: { label: 'Waiting for reply', icon: MessageCircleQuestion, className: 'text-amber-500' },
  done: { label: 'Done', icon: CheckCircle2, className: 'text-emerald-500' },
  failed: { label: 'Failed', icon: OctagonX, className: 'text-destructive' },
  manual: { label: 'Manual', icon: UserRound, className: 'text-purple-500' },
};

/// Auto-resolve queue: every tracked GitHub issue with its live status.
/// Rows bound to a task open that chat on click.
export function IssueQueueDialog({ open, onClose }) {
  const [issues, setIssues] = useState([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!open) return;
    let active = true;
    const refresh = () => {
      invoke('github_auto_list_issues', { projectId: null })
        .then((rows) => { if (active) { setIssues(rows || []); setLoading(false); } })
        .catch(() => { if (active) setLoading(false); });
    };
    setLoading(true);
    refresh();
    // Live updates while the dialog is open — the worker + webhook publish
    // these on every queue mutation.
    const unsubs = [];
    listen('github-queue-changed', refresh).then((u) => unsubs.push(u));
    listen('github-issue-updated', refresh).then((u) => unsubs.push(u));
    return () => {
      active = false;
      unsubs.forEach((u) => { try { u(); } catch { /* already torn down */ } });
    };
  }, [open]);

  const openChat = (row) => {
    if (!row.task_id) {
      toast.info('No chat yet — the issue has not been picked up by the worker.');
      return;
    }
    useAgent.getState().setActiveTask(row.task_id);
    useLayout.getState().openChatDock();
    onClose();
  };

  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) onClose(); }}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>GitHub issue queue</DialogTitle>
          <DialogDescription>
            Issues auto-tracked from connected repos. Click one to open its chat.
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[55vh] overflow-y-auto -mx-1 px-1">
          {loading && issues.length === 0 && (
            <div className="py-8 text-center text-xs text-muted-foreground">Loading…</div>
          )}
          {!loading && issues.length === 0 && (
            <div className="py-8 text-center text-xs text-muted-foreground">
              No issues tracked yet. Label an issue on a connected repo to start.
            </div>
          )}
          <ul className="space-y-1">
            {issues.map((row) => {
              const meta = STATUS_META[row.status] || STATUS_META.queued;
              const Icon = meta.icon;
              return (
                <li key={row.id}>
                  <button
                    type="button"
                    onClick={() => openChat(row)}
                    className="w-full rounded-md border border-border/50 bg-muted/20 px-3 py-2 text-left hover:bg-muted/40 transition-colors"
                  >
                    <div className="flex items-center gap-2">
                      <Icon className={cn('size-3.5 shrink-0', meta.className, meta.spin && 'animate-spin')} />
                      <span className="text-[13px] font-medium truncate flex-1">
                        #{row.issue_number} — {row.title || '(untitled)'}
                      </span>
                      <Badge variant="outline" className={cn('h-5 text-[10px] shrink-0', meta.className)}>
                        {meta.label}
                      </Badge>
                    </div>
                    <div className="mt-0.5 flex items-center gap-2 text-[11px] text-muted-foreground">
                      <span className="font-mono truncate">{row.repo_full_name}</span>
                      {row.error && (
                        <span className="truncate text-destructive/80" title={row.error}>· {row.error}</span>
                      )}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export default IssueQueueDialog;
