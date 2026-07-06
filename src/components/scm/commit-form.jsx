import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, ChevronDown, Loader2, Sparkles, UploadCloud } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { toast } from 'sonner';
import { confirm } from '@/components/confirm-dialog';
import { useGit } from '@/state/git';
import { useLayout } from '@/state/layout';

export default function CommitForm({ projectId }) {
  const message = useGit((s) => s.commitMessages[projectId] ?? '');
  const setMessage = useGit((s) => s.setCommitMessage);

  // TRUE per-category totals from the backend — NOT the lengths of the
  // windowed row arrays. The status list only ships the first N rows of a
  // huge change list ("Load more" pages the rest in), so counting the arrays
  // here made commit think a 90k-file change set was 500 files.
  const stagedTotal = useGit(
    (s) => s.projects[projectId]?.statusCounts?.staged ?? 0
  );
  const changesTotal = useGit((s) => {
    const c = s.projects[projectId]?.statusCounts;
    return (c?.unstaged ?? 0) + (c?.untracked ?? 0);
  });

  const stageAll = useGit((s) => s.stageAll);
  const commit = useGit((s) => s.commit);
  const commitAndPush = useGit((s) => s.commitAndPush);
  const [submitting, setSubmitting] = useState(false);

  // Live progress from the backend's `git-progress` events: staging a huge
  // tree streams "N files staged so far"; committing flags the phase. Without
  // this the only signal during a multi-minute `git add -A` was a frozen
  // button.
  const [progress, setProgress] = useState(null);
  useEffect(() => {
    let alive = true;
    let unlisten;
    (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        const un = await listen('git-progress', (e) => {
          const p = e.payload ?? {};
          if (p.projectId !== projectId || !alive) return;
          setProgress(p.phase === 'done' ? null : p);
        });
        if (!alive) un();
        else unlisten = un;
      } catch {
        // Event transport unavailable — progress line just stays generic.
      }
    })();
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [projectId]);

  const totalChanges = stagedTotal + changesTotal;
  const canCommit = !!projectId && totalChanges > 0 && message.trim().length > 0 && !submitting;

  const [generating, setGenerating] = useState(false);
  const canGenerate = !!projectId && totalChanges > 0 && !generating && !submitting;

  /// Generate an AI commit message, prompting to configure a model if none is set.
  async function handleGenerate() {
    if (!canGenerate) return;
    let hasModel = false;
    try {
      const cfg = await invoke('get_ai_config');
      const sc = cfg?.source_control;
      hasModel = !!(sc && sc.provider_key && sc.model);
    } catch {
      hasModel = false;
    }

    if (!hasModel) {
      const ok = await confirm({
        title: 'Set up commit-message AI',
        description:
          'No model is configured for generating commit messages. Pick a connected provider and model in Settings → Agent → Source Control, then try again.',
        confirmLabel: 'Open Settings',
        cancelLabel: 'Not now',
      });
      if (ok) useLayout.getState().openSettingsAt('agent-models', 'Source Control');
      return;
    }

    setGenerating(true);
    try {
      const res = await invoke('generate_commit_message', { projectId });
      const text = typeof res === 'string' ? res : res?.message ?? '';
      if (text.trim()) setMessage(projectId, text.trim());
      else toast.error('The model returned an empty message.');
    } catch (err) {
      toast.error(`Couldn't generate a message: ${err}`);
    } finally {
      setGenerating(false);
    }
  }

  // If nothing is staged, stage EVERYTHING first (VS Code behaviour) — via
  // the repo-wide `git add -A`, not by enumerating the loaded rows. The old
  // path-list version only staged the windowed first page, silently capping
  // commits at ~500 files on big change lists.
  async function ensureStaged() {
    if (stagedTotal > 0) return;
    if (changesTotal === 0) return;
    await stageAll(projectId);
  }

  async function handleCommit() {
    if (!canCommit) return;
    setSubmitting(true);
    try {
      await ensureStaged();
      const hash = await commit(projectId);
      if (hash) toast.success(`Committed ${String(hash).slice(0, 7)}`);
    } catch (err) {
      toast.error(`Commit failed: ${err}`, {
        action: { label: 'Retry', onClick: () => handleCommit() },
      });
    } finally {
      setSubmitting(false);
      setProgress(null);
    }
  }

  async function handleCommitAndPush() {
    if (!canCommit) return;
    setSubmitting(true);
    try {
      await ensureStaged();
      const hash = await commitAndPush(projectId);
      if (hash) toast.success(`Committed & pushed ${String(hash).slice(0, 7)}`);
    } catch (err) {
      toast.error(`Commit & push failed: ${err}`, {
        action: { label: 'Retry', onClick: () => handleCommitAndPush() },
      });
    } finally {
      setSubmitting(false);
      setProgress(null);
    }
  }

  const placeholder = stagedTotal > 0
    ? `Message (${stagedTotal.toLocaleString()} staged)`
    : totalChanges > 0
      ? `Message (${totalChanges.toLocaleString()} changes — will stage all)`
      : 'No changes to commit';

  // Network phases (push/pull/fetch/publish) carry git's own sideband line —
  // "Receiving objects: 42% (12000/90000)", "Updating files: 18% (…)" — show
  // it verbatim; it's the richest status git can give us.
  const NETWORK_PHASE_LABEL = {
    pushing: 'Pushing',
    pulling: 'Pulling',
    fetching: 'Fetching',
    publishing: 'Publishing',
  };
  const progressText = progress?.phase === 'staging'
    ? `Staging files… ${(progress.done ?? 0).toLocaleString()}${changesTotal > 0 ? ` / ${changesTotal.toLocaleString()}` : ''}`
    : progress?.phase === 'committing'
      ? `Committing${stagedTotal > 0 ? ` ${stagedTotal.toLocaleString()} files` : ''}…`
      : NETWORK_PHASE_LABEL[progress?.phase]
        ? (progress.text || `${NETWORK_PHASE_LABEL[progress.phase]}…`)
        : 'Working…';

  return (
    <div className="flex w-full min-w-0 flex-col gap-1.5 px-2 py-2">
      <div className="relative w-full">
        <Textarea
          value={message}
          onChange={(e) => setMessage(projectId, e.target.value)}
          placeholder={placeholder}
          rows={2}
          className="min-h-[44px] w-full max-w-full resize-none pr-8 text-xs [field-sizing:fixed]"
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
              e.preventDefault();
              handleCommit();
            }
          }}
        />
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              size="icon-sm"
              variant="ghost"
              disabled={!canGenerate}
              onClick={handleGenerate}
              aria-label="Generate commit message with AI"
              className="absolute right-1 top-1 size-6 text-muted-foreground hover:text-primary"
            >
              {generating ? <Loader2 className="size-3.5 animate-spin" /> : <Sparkles className="size-3.5" />}
            </Button>
          </TooltipTrigger>
          <TooltipContent>Generate commit message with AI</TooltipContent>
        </Tooltip>
      </div>
      <div className="flex w-full">
        <Button
          size="sm"
          disabled={!canCommit}
          onClick={handleCommit}
          className="h-7 flex-1 rounded-r-none hover:bg-primary/80 active:bg-primary/70"
        >
          {submitting ? <Loader2 className="animate-spin" /> : <Check />}
          Commit
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              size="sm"
              disabled={!canCommit}
              className="h-7 w-7 shrink-0 rounded-l-none border-l border-primary-foreground/20 px-0 hover:bg-primary/80 active:bg-primary/70"
              aria-label="More commit options"
            >
              <ChevronDown className="size-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-[160px]">
            <DropdownMenuItem onClick={handleCommit} disabled={!canCommit}>
              <Check className="size-3" />
              Commit
            </DropdownMenuItem>
            <DropdownMenuItem onClick={handleCommitAndPush} disabled={!canCommit}>
              <UploadCloud className="size-3" />
              Commit &amp; Push
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {(progress || submitting) && (
        <div className="flex items-center gap-1.5 px-0.5 text-[11px] text-muted-foreground">
          <Loader2 className="size-3 shrink-0 animate-spin" />
          <span className="truncate">{progress ? progressText : 'Working…'}</span>
        </div>
      )}
    </div>
  );
}
