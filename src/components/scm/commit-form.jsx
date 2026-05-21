import React, { useState } from 'react';
import { Check, ChevronDown, Loader2, UploadCloud } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { toast } from 'sonner';
import { useGit } from '@/state/git';

export default function CommitForm({ projectId }) {
  const message = useGit((s) => s.commitMessages[projectId] ?? '');
  const setMessage = useGit((s) => s.setCommitMessage);

  const stagedCount = useGit(
    (s) => s.projects[projectId]?.status?.staged?.length ?? 0
  );
  const unstagedCount = useGit((s) => {
    const p = s.projects[projectId];
    return (p?.status?.unstaged?.length ?? 0) + (p?.status?.untracked?.length ?? 0);
  });

  const stage = useGit((s) => s.stage);
  const commit = useGit((s) => s.commit);
  const commitAndPush = useGit((s) => s.commitAndPush);
  const [submitting, setSubmitting] = useState(false);

  const totalChanges = stagedCount + unstagedCount;
  const canCommit = !!projectId && totalChanges > 0 && message.trim().length > 0 && !submitting;

  // If nothing is staged, auto-stage all changes first (VS Code behaviour).
  async function ensureStaged() {
    if (stagedCount > 0) return;
    const status = useGit.getState().projects[projectId]?.status;
    const paths = [
      ...(status?.unstaged ?? []),
      ...(status?.untracked ?? []),
    ].map((f) => f.path ?? f.file ?? '').filter(Boolean);
    if (paths.length > 0) await stage(paths, projectId);
  }

  async function handleCommit() {
    if (!canCommit) return;
    setSubmitting(true);
    try {
      await ensureStaged();
      const hash = await commit(projectId);
      if (hash) toast.success(`Committed ${String(hash).slice(0, 7)}`);
    } catch (err) {
      toast.error(`Commit failed: ${err}`);
    } finally {
      setSubmitting(false);
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
      toast.error(`Commit & push failed: ${err}`);
    } finally {
      setSubmitting(false);
    }
  }

  const placeholder = stagedCount > 0
    ? `Message (${stagedCount} staged)`
    : totalChanges > 0
      ? `Message (${totalChanges} changes — will stage all)`
      : 'No changes to commit';

  return (
    <div className="flex w-full min-w-0 flex-col gap-1.5 px-2 py-2">
      <Textarea
        value={message}
        onChange={(e) => setMessage(projectId, e.target.value)}
        placeholder={placeholder}
        rows={2}
        className="min-h-[44px] w-full max-w-full resize-none text-xs [field-sizing:fixed]"
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
            e.preventDefault();
            handleCommit();
          }
        }}
      />
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
    </div>
  );
}
