import React, { useCallback, useEffect, useState } from 'react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import {
  ChevronDown,
  ChevronRight,
  ArrowUp,
  ArrowDown,
  Plus,
  Minus,
  Check,
  ExternalLink,
  MoreHorizontal,
  Loader2,
  FolderGit2,
  ListCollapse,
  RefreshCw,
  CloudUpload,
  GitFork,
  Lock,
  Globe,
  Undo2,
  LogOut,
} from 'lucide-react';
import { GithubIcon } from '@/components/github/icon';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { ScrollArea } from '@/components/ui/scroll-area';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { toast } from 'sonner';
import { confirm } from '@/components/confirm-dialog';
import { useGit } from '@/state/git';
import { useExplorer } from '@/state/explorer';
import { useEditor } from '@/state/editor';
import { useGithubAuth } from '@/state/github';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { AddProjectButton } from '@/components/shell/add-project-button';
import { SortableProjectList, useProjectSortable, ProjectDragHandle } from '@/components/shell/sortable-projects';
import FileChangeItem from './file-change-item';
import CommitForm from './commit-form';
import BranchSwitcher from './branch-switcher';
import CommitHistory from './commit-history';
import ConflictPanel from './conflict-panel';

// ── GitHub header button (sign in / account) ──────────────────────────

function GithubHeaderButton() {
  const user = useGithubAuth((s) => s.user);
  const hasToken = useGithubAuth((s) => s.hasToken);
  const openDialog = useGithubAuth((s) => s.openDialog);
  const signOut = useGithubAuth((s) => s.signOut);

  // Not signed in — single click opens the auth dialog.
  if (!user && !hasToken) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-xs" onClick={openDialog}>
            <GithubIcon className="size-3" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
          Sign in to GitHub
        </TooltipContent>
      </Tooltip>
    );
  }

  const label = user?.login ?? 'GitHub';
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-xs">
              <GithubIcon className="size-3" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
          {`Signed in as ${label}`}
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="min-w-[180px]">
        <DropdownMenuItem
          onClick={() =>
            user?.login
              ? openUrl(`https://github.com/${user.login}`).catch(() => {})
              : openDialog()
          }
          className="whitespace-nowrap"
        >
          <GithubIcon className="size-3" />
          {label}
          {user?.login && <ExternalLink className="ml-auto size-3 text-muted-foreground" />}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={openDialog} className="whitespace-nowrap">
          Switch account…
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={signOut}
          className="whitespace-nowrap text-destructive focus:text-destructive"
        >
          <LogOut className="size-3" />
          Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ── Section (collapsible sub-section) ─────────────────────────────────

function Section({ id, title, count, children, actions }) {
  const expanded = useGit((s) => s.expanded[id] ?? false);
  const toggle = useGit((s) => s.toggleSection);
  return (
    <div className="flex flex-col overflow-hidden">
      <div className="flex h-7 items-center gap-1 overflow-hidden px-3 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
        <button
          type="button"
          onClick={() => toggle(id)}
          className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden hover:text-foreground/80"
        >
          {expanded ? (
            <ChevronDown className="size-3 shrink-0" />
          ) : (
            <ChevronRight className="size-3 shrink-0" />
          )}
          <span className="truncate">{title}</span>
          {typeof count === 'number' && count > 0 && (
            <span className="ml-1 shrink-0 rounded bg-muted px-1 py-px text-[10px] font-normal normal-case text-foreground">
              {count}
            </span>
          )}
        </button>
        {actions && (
          <div className="flex shrink-0 items-center gap-0.5">{actions}</div>
        )}
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateRows: expanded ? '1fr' : '0fr',
          transition: 'grid-template-rows 200ms cubic-bezier(0.4, 0, 0.2, 1)',
        }}
      >
        <div style={{ overflow: 'hidden' }}>
          <div className="flex flex-col">{children}</div>
        </div>
      </div>
    </div>
  );
}

// ── Publish to GitHub dialog ───────────────────────────────────────────

function PublishToGitHubDialog({ open, onOpenChange, defaultName, projectId }) {
  const publishToGitHub = useGit((s) => s.publishToGitHub);
  const [repoName, setRepoName] = useState(defaultName ?? '');
  const [isPrivate, setIsPrivate] = useState(true);
  const [publishing, setPublishing] = useState(false);

  // Sync defaultName when the dialog opens
  useEffect(() => {
    if (open) setRepoName(defaultName ?? '');
  }, [open, defaultName]);

  async function handlePublish() {
    const name = repoName.trim();
    if (!name) return;
    setPublishing(true);
    try {
      await publishToGitHub(projectId, name, isPrivate);
      toast.success('Published to GitHub');
      onOpenChange(false);
    } catch (err) {
      toast.error(`Publish failed: ${err}`);
    } finally {
      setPublishing(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[400px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <CloudUpload className="size-4" />
            Publish to GitHub
          </DialogTitle>
        </DialogHeader>

        <div className="flex flex-col gap-4 py-2">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="repo-name">Repository name</Label>
            <Input
              id="repo-name"
              value={repoName}
              onChange={(e) => setRepoName(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && !publishing && handlePublish()}
              placeholder="my-project"
              autoFocus
            />
          </div>

          <div className="flex flex-col gap-2">
            <Label>Visibility</Label>
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => setIsPrivate(true)}
                className={cn(
                  'flex flex-1 items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors',
                  isPrivate
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'border-border text-muted-foreground hover:border-border/80 hover:text-foreground'
                )}
              >
                <Lock className="size-3.5 shrink-0" />
                <span className="font-medium">Private</span>
              </button>
              <button
                type="button"
                onClick={() => setIsPrivate(false)}
                className={cn(
                  'flex flex-1 items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors',
                  !isPrivate
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'border-border text-muted-foreground hover:border-border/80 hover:text-foreground'
                )}
              >
                <Globe className="size-3.5 shrink-0" />
                <span className="font-medium">Public</span>
              </button>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={publishing}>
            Cancel
          </Button>
          <Button onClick={handlePublish} disabled={publishing || !repoName.trim()}>
            {publishing ? (
              <>
                <Loader2 className="size-3.5 animate-spin" />
                Publishing…
              </>
            ) : (
              <>
                <CloudUpload className="size-3.5" />
                Publish
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ── Per-project SCM section ────────────────────────────────────────────

// How many file rows to render per section before requiring "Load more". A repo
// where node_modules (or any huge tree) got staged can carry tens of thousands
// of entries; rendering them all at once freezes the whole IDE. We window the
// list, show the TRUE total in the section header, and reveal the next chunk on
// demand. 500 is comfortably below where row rendering starts to lag.
const SCM_PAGE_SIZE = 500;

// A clickable footer row that reveals the next chunk of a long file list.
function LoadMoreRow({ remaining, onClick }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex w-full items-center justify-center gap-1 px-2 py-1.5 text-[11px] text-muted-foreground hover:bg-muted hover:text-foreground"
    >
      Load {Math.min(SCM_PAGE_SIZE, remaining)} more
      <span className="text-muted-foreground/70">({remaining.toLocaleString()} hidden)</span>
    </button>
  );
}

function ProjectScmSection({ project }) {
  const projectId = project.id;
  const projectName = project.name ?? project.root_path?.split(/[\\/]/).pop() ?? projectId;

  const gitProject = useGit((s) => s.projects[projectId]);
  const loading = gitProject?.loading ?? false;
  const status = gitProject?.status ?? { unstaged: [], staged: [], untracked: [] };
  const statusCounts = gitProject?.statusCounts ?? { staged: 0, unstaged: 0, untracked: 0 };
  const aheadBehind = gitProject?.aheadBehind ?? { ahead: 0, behind: 0 };
  const log = gitProject?.log ?? [];
  const isGitRepo = gitProject?.isGitRepo ?? null;
  const remoteUrl = gitProject?.remoteUrl ?? null;

  const [publishDialogOpen, setPublishDialogOpen] = useState(false);
  const [syncing, setSyncing] = useState(null);

  const expanded = useGit((s) => s.expanded[`project-${projectId}`] ?? false);
  const toggle = useGit((s) => s.toggleSection);

  const { setNodeRef, style: sortableStyle, dragHandleProps } = useProjectSortable(projectId);

  const refreshAll = useGit((s) => s.refreshAll);
  const initRepo = useGit((s) => s.initRepo);
  const stage = useGit((s) => s.stage);
  const unstage = useGit((s) => s.unstage);
  const discard = useGit((s) => s.discard);
  const stageAll = useGit((s) => s.stageAll);
  const unstageAll = useGit((s) => s.unstageAll);
  const discardAll = useGit((s) => s.discardAll);
  const loadMoreStatus = useGit((s) => s.loadMoreStatus);
  const push = useGit((s) => s.push);
  const pull = useGit((s) => s.pull);
  const fetch = useGit((s) => s.fetch);
  const sync = useGit((s) => s.sync);
  const undoLastCommit = useGit((s) => s.undoLastCommit);

  // Only fetch when the accordion is expanded — avoids loading every project
  // on mount and avoids redundant re-fetches when revisiting the SCM panel.
  const alreadyFetched = gitProject != null && isGitRepo !== null;

  useEffect(() => {
    if (!expanded) return;
    if (alreadyFetched) return;
    refreshAll(projectId);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expanded, projectId]);

  // If the section is expanded but isGitRepo was never resolved (e.g. stale
  // state from before the field existed), trigger a recovery refresh.
  useEffect(() => {
    if (!expanded) return;
    if (isGitRepo === null && !loading) refreshAll(projectId);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expanded, isGitRepo, loading, projectId]);

  // `status.*` is a windowed slice the backend caps at the per-project limit;
  // the real totals come from statusCounts. The "Changes" section merges
  // unstaged + untracked, so its total is the sum.
  const unstaged = [...(status.unstaged ?? []), ...(status.untracked ?? [])];
  const staged = status.staged ?? [];
  const stagedTotal = statusCounts.staged;
  const changesTotal = statusCounts.unstaged + statusCounts.untracked;

  async function withToast(fn, success, fail) {
    try {
      await fn();
      if (success) toast.success(success);
    } catch (err) {
      toast.error(`${fail}: ${err}`, {
        action: { label: 'Retry', onClick: () => withToast(fn, success, fail) },
      });
    }
  }

  async function handleQuickPush(e) {
    e.stopPropagation();
    if (syncing) return;
    setSyncing('push');
    try {
      await withToast(() => push(projectId), 'Pushed', 'Push failed');
    } finally {
      setSyncing(null);
    }
  }

  async function handleQuickPull(e) {
    e.stopPropagation();
    if (syncing) return;
    setSyncing('pull');
    try {
      await withToast(() => pull(projectId), 'Pulled', 'Pull failed');
    } finally {
      setSyncing(null);
    }
  }

  const openDiff = useCallback((file, oid, title) => {
    useEditor.getState().openDiff({
      projectId,
      filePath: file.path ?? file.file,
      ...(oid ? { oid } : {}),
      ...(title ? { title } : {}),
    });
  }, [projectId]);

  const handleStage   = useCallback((p) => stage([p], projectId),   [stage, projectId]);
  const handleUnstage = useCallback((p) => unstage([p], projectId), [unstage, projectId]);
  const handleDiscard = useCallback((p) => discard([p], projectId), [discard, projectId]);

  // Discard all unstaged + untracked changes for this project. Destructive, so
  // gate behind the shared confirm dialog.
  const handleDiscardAll = useCallback(async () => {
    if (changesTotal === 0) return;
    const ok = await confirm({
      title: `Discard all changes in ${projectName}?`,
      description: `This will permanently revert ${changesTotal.toLocaleString()} file${changesTotal === 1 ? '' : 's'} to their last committed state and delete any new untracked files. This cannot be undone.`,
      confirmLabel: 'Discard all',
      destructive: true,
    });
    if (!ok) return;
    await withToast(
      () => discardAll(projectId),
      `Discarded ${changesTotal.toLocaleString()} change${changesTotal === 1 ? '' : 's'}`,
      'Discard failed'
    );
  }, [changesTotal, projectId, projectName, discardAll]);

  return (
    <div ref={setNodeRef} style={sortableStyle} className="flex w-full min-w-0 flex-col overflow-hidden border-b border-border/60 last:border-b-0">
      {/* Explorer-style sticky project header */}
      <div className="group/project sticky top-0 z-10 flex h-7 w-full items-center gap-1 overflow-hidden border-b border-border/60 bg-muted/60 px-2 backdrop-blur">
        <ProjectDragHandle dragHandleProps={dragHandleProps} />
        <button
          type="button"
          onClick={() => toggle(`project-${projectId}`)}
          className="flex min-w-0 flex-1 items-center gap-1.5 overflow-hidden text-left text-[11px] font-semibold uppercase tracking-wide text-foreground/90 hover:text-foreground"
        >
          <ChevronRight
            className="size-3 shrink-0 transition-transform duration-200 ease-in-out"
            style={{ transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)' }}
          />
          <FolderGit2 className="size-3 shrink-0" />
          <span className="min-w-0 truncate">{projectName}</span>
          {loading && <Loader2 className="size-3 shrink-0 animate-spin text-muted-foreground" />}
        </button>
        {!loading && aheadBehind.ahead > 0 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={handleQuickPush}
                disabled={syncing !== null}
                className="flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 text-[10px] font-normal normal-case text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-50"
              >
                {syncing === 'push' ? (
                  <Loader2 className="size-2.5 animate-spin" />
                ) : (
                  <ArrowUp className="size-2.5" />
                )}
                {aheadBehind.ahead}
              </button>
            </TooltipTrigger>
            <TooltipContent>Push {aheadBehind.ahead} commit{aheadBehind.ahead === 1 ? '' : 's'}</TooltipContent>
          </Tooltip>
        )}
        {!loading && aheadBehind.behind > 0 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={handleQuickPull}
                disabled={syncing !== null}
                className="flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 text-[10px] font-normal normal-case text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-50"
              >
                {syncing === 'pull' ? (
                  <Loader2 className="size-2.5 animate-spin" />
                ) : (
                  <ArrowDown className="size-2.5" />
                )}
                {aheadBehind.behind}
              </button>
            </TooltipTrigger>
            <TooltipContent>Pull {aheadBehind.behind} commit{aheadBehind.behind === 1 ? '' : 's'}</TooltipContent>
          </Tooltip>
        )}

        {/* Branch switcher + actions dropdown — only visible when expanded */}
        {expanded && (
          <div className="flex shrink-0 items-center gap-0.5">
            <BranchSwitcher projectId={projectId} className="max-w-[110px]" />
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="icon-xs">
                  <MoreHorizontal />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="min-w-[170px]">
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => withToast(() => push(projectId), 'Pushed', 'Push failed')}
                >
                  <ArrowUp className="size-3" />
                  Push{aheadBehind.ahead > 0 ? ` (${aheadBehind.ahead})` : ''}
                </DropdownMenuItem>
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => withToast(() => pull(projectId), 'Pulled', 'Pull failed')}
                >
                  <ArrowDown className="size-3" />
                  Pull{aheadBehind.behind > 0 ? ` (${aheadBehind.behind})` : ''}
                </DropdownMenuItem>
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => withToast(() => fetch(projectId), 'Fetched', 'Fetch failed')}
                >
                  Fetch
                </DropdownMenuItem>
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => withToast(() => sync(projectId), 'Synced', 'Sync failed')}
                >
                  Sync (Pull + Push)
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => withToast(() => stageAll(projectId), 'Staged all changes', 'Stage failed')}
                  disabled={changesTotal === 0}
                >
                  Stage all changes
                </DropdownMenuItem>
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => withToast(() => unstageAll(projectId), 'Unstaged all', 'Unstage failed')}
                  disabled={stagedTotal === 0}
                >
                  Unstage all
                </DropdownMenuItem>
                <DropdownMenuItem
                  className="whitespace-nowrap text-destructive focus:text-destructive"
                  onClick={handleDiscardAll}
                  disabled={changesTotal === 0}
                >
                  <Undo2 className="size-3" />
                  Discard all changes
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() =>
                    withToast(() => undoLastCommit(projectId), 'Undid last commit', 'Undo failed')
                  }
                >
                  Undo last commit
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={() => refreshAll(projectId)} disabled={loading}
                >
                  Refresh
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        )}
      </div>

      {/* Animated expand/collapse */}
      <div
        style={{
          display: 'grid',
          gridTemplateRows: expanded ? '1fr' : '0fr',
          transition: 'grid-template-rows 200ms ease',
          width: '100%',
          minWidth: 0,
        }}
      >
        <div style={{ overflow: 'hidden', minWidth: 0 }}>
          {/* ── Not a git repo: show Initialize Repository ── */}
          {isGitRepo === false && (
            <div className="flex flex-col items-center gap-3 px-4 py-6 text-center">
              <GitFork className="size-7 text-muted-foreground/60" />
              <div className="flex flex-col gap-1">
                <p className="text-xs font-medium text-foreground">Not a Git repository</p>
                <p className="text-[11px] text-muted-foreground">
                  Initialize to track changes with Git.
                </p>
              </div>
              <Button
                size="sm"
                className="h-7 px-3 text-xs"
                onClick={() =>
                  withToast(
                    () => initRepo(projectId),
                    'Repository initialized',
                    'Initialize failed'
                  )
                }
                disabled={loading}
              >
                {loading ? <Loader2 className="size-3 animate-spin" /> : null}
                Initialize Repository
              </Button>
            </div>
          )}

          {/* ── Git repo but no remote: show Publish to GitHub banner ── */}
          {isGitRepo === true && remoteUrl === null && (
            <div className="flex items-center gap-2 border-b border-border/60 bg-muted/40 px-3 py-2">
              <CloudUpload className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
                No remote configured
              </span>
              <Button
                size="sm"
                variant="outline"
                className="h-6 shrink-0 px-2 text-[11px]"
                onClick={() => setPublishDialogOpen(true)}
              >
                Publish to GitHub
              </Button>
            </div>
          )}

          {/* ── Normal SCM content (only when repo is confirmed initialized) ── */}
          {isGitRepo === true && (
            <>
              <ConflictPanel
                projectId={projectId}
                onOpenEditor={(path) =>
                  useEditor.getState().openFile({ projectId, filePath: path })
                }
              />

              <CommitForm projectId={projectId} />

              {!loading && stagedTotal === 0 && changesTotal === 0 && (
                <div className="flex items-center gap-1.5 px-3 py-1.5 text-[11px] text-muted-foreground">
                  <Check className="size-3 text-emerald-500" />
                  No changes
                </div>
              )}

              {/* Staged Changes — hidden when empty */}
              {stagedTotal > 0 && (
                <Section
                  id={`${projectId}-staged`}
                  title="Staged Changes"
                  count={stagedTotal}
                  actions={
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          onClick={() => unstageAll(projectId)}
                        >
                          <Minus />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>Unstage all</TooltipContent>
                    </Tooltip>
                  }
                >
                  {staged.map((f, i) => (
                    <FileChangeItem
                      key={(f.path ?? f.file ?? '') + i}
                      file={f}
                      staged
                      onUnstage={handleUnstage}
                      onOpenDiff={openDiff}
                    />
                  ))}
                  {staged.length < stagedTotal && (
                    <LoadMoreRow
                      remaining={stagedTotal - staged.length}
                      onClick={() => loadMoreStatus(projectId)}
                    />
                  )}
                </Section>
              )}

              {/* Changes — hidden when empty */}
              {changesTotal > 0 && (
                <Section
                  id={`${projectId}-changes`}
                  title="Changes"
                  count={changesTotal}
                  actions={
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon-xs"
                            onClick={handleDiscardAll}
                          >
                            <Undo2 />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>Discard all</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon-xs"
                            onClick={() => stageAll(projectId)}
                          >
                            <Plus />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>Stage all</TooltipContent>
                      </Tooltip>
                    </>
                  }
                >
                  {unstaged.map((f, i) => (
                    <FileChangeItem
                      key={(f.path ?? f.file ?? '') + i}
                      file={f}
                      onStage={handleStage}
                      onDiscard={handleDiscard}
                      onOpenDiff={openDiff}
                    />
                  ))}
                  {unstaged.length < changesTotal && (
                    <LoadMoreRow
                      remaining={changesTotal - unstaged.length}
                      onClick={() => loadMoreStatus(projectId)}
                    />
                  )}
                </Section>
              )}

              {/* Commits / Graph — hidden when no history yet */}
              {log.length > 0 && (
                <Section id={`${projectId}-history`} title="Commits">
                  <CommitHistory
                    projectId={projectId}
                    onSelectFile={(file) =>
                      openDiff(
                        file,
                        file.commitOid,
                        `Δ ${(file.path ?? file.file ?? '').split(/[\\/]/).pop()} @ ${(file.commitOid ?? '').slice(0, 7)}`
                      )
                    }
                  />
                </Section>
              )}
            </>
          )}

          {/* Bottom gap — matching the Explorer's spacing */}
          {expanded && <div className="h-16 w-full" />}
        </div>
      </div>

      <PublishToGitHubDialog
        open={publishDialogOpen}
        onOpenChange={setPublishDialogOpen}
        defaultName={projectName}
        projectId={projectId}
      />
    </div>
  );
}

// ── Root SCM Panel ─────────────────────────────────────────────────────

export default function ScmPanel() {
  const projects = useExplorer((s) => s.projects);
  const loadProjects = useExplorer((s) => s.loadProjects);
  const hasLoaded = useExplorer((s) => s.hasLoaded);
  const collapseAllProjects = useGit((s) => s.collapseAllProjects);
  const refreshAll = useGit((s) => s.refreshAll);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    if (!hasLoaded) loadProjects();
  }, [hasLoaded, loadProjects]);

  async function handleRefreshAll() {
    setRefreshing(true);
    const minDelay = new Promise((r) => setTimeout(r, 600));
    try {
      await Promise.all([...projects.map((p) => refreshAll(p.id)), minDelay]);
    } finally {
      setRefreshing(false);
    }
  }

  const header = (
    <div className="flex h-8 shrink-0 items-center gap-2 overflow-hidden border-b border-border/60 px-2">
      <span className="min-w-0 flex-1 truncate text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
        Source Control
      </span>
      <div className="flex shrink-0 items-center gap-1">
        <GithubHeaderButton />
        <AddProjectButton />
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              onClick={() => collapseAllProjects(projects.map((p) => p.id))}
            >
              <ListCollapse className="size-3" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
            Collapse All
          </TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              onClick={handleRefreshAll}
              disabled={refreshing}
            >
              <RefreshCw className={cn('size-3', refreshing && 'animate-spin')} />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" align="end" sideOffset={4} className="px-2 py-1">
            Refresh All
          </TooltipContent>
        </Tooltip>
      </div>
    </div>
  );

  if (projects.length === 0) {
    return (
      <div className="flex h-full flex-col overflow-hidden">
        {header}
        <div className="flex flex-1 items-center justify-center px-6 text-center text-xs text-muted-foreground">
          No projects open. Open a folder to use Source Control.
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col overflow-hidden">
      {header}

      <ScrollArea className="min-h-0 flex-1">
        <div className="flex w-full min-w-0 flex-col">
          <SortableProjectList projects={projects}>
            {projects.map((p) => (
              <ProjectScmSection key={p.id} project={p} />
            ))}
          </SortableProjectList>
        </div>
      </ScrollArea>

    </div>
  );
}
