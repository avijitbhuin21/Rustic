import React from 'react';
import { AlertCircle, FileEdit, LogOut, Loader2 } from 'lucide-react';
import { GithubIcon } from '@/components/github/icon';
import { useGit } from '@/state/git';
import { useEditor } from '@/state/editor';
import { useGithubAuth } from '@/state/github';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuSeparator,
  DropdownMenuLabel,
} from '@/components/ui/dropdown-menu';

function GithubStatusItem() {
  const user = useGithubAuth((s) => s.user);
  const hasToken = useGithubAuth((s) => s.hasToken);
  const loading = useGithubAuth((s) => s.loading);
  const openDialog = useGithubAuth((s) => s.openDialog);
  const signOut = useGithubAuth((s) => s.signOut);

  // Not signed in — single click opens the sign-in dialog.
  if (!user && !hasToken) {
    return (
      <button
        type="button"
        onClick={openDialog}
        className="flex items-center gap-1 px-1 hover:text-foreground"
        title="Sign in to GitHub"
      >
        {loading ? <Loader2 className="size-3 animate-spin" /> : <GithubIcon className="size-3" />}
        <span>Sign in to GitHub</span>
      </button>
    );
  }

  // Token present but user not yet fetched (transient): show generic icon.
  const label = user?.login ?? 'GitHub';

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex items-center gap-1 px-1 hover:text-foreground aria-expanded:text-foreground"
          title={`Signed in as ${label}`}
        >
          <GithubIcon className="size-3" />
          <span>{label}</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="min-w-[180px]">
        <DropdownMenuLabel className="flex items-center gap-2">
          <GithubIcon className="size-3.5" />
          <span className="truncate">{label}</span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={openDialog}>
          Switch account…
        </DropdownMenuItem>
        <DropdownMenuItem
          className="text-destructive focus:text-destructive"
          onClick={signOut}
        >
          <LogOut className="size-3" />
          Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function StatusBar() {
  const projectGit = useGit((s) => s.projects[s.activeProjectId]);
  const groups      = useEditor((s) => s.groups);
  const activeGroupId = useEditor((s) => s.activeGroupId);
  const cursor      = useEditor((s) => s.cursor);

  const allTabs   = (groups ?? []).flatMap((g) => g.tabs);
  const dirtyCount = allTabs.filter((t) => t.dirty).length;
  const activeGroup = groups.find((g) => g.id === activeGroupId);
  const activeTab   = activeGroup?.tabs.find((t) => t.id === activeGroup.activeId) ?? null;
  const conflicts = projectGit?.conflicts?.length ?? 0;

  return (
    <div className="flex h-6 shrink-0 items-center justify-between border-t border-border bg-background px-2 text-[11px] text-muted-foreground select-none">
      <div className="flex items-center gap-3">
        <GithubStatusItem />
        {conflicts > 0 && (
          <span className="flex items-center gap-1 text-destructive">
            <AlertCircle className="size-3" />
            {conflicts} conflict{conflicts === 1 ? '' : 's'}
          </span>
        )}
        {dirtyCount > 0 && (
          <span className="flex items-center gap-1 text-foreground">
            <FileEdit className="size-3" />
            {dirtyCount} unsaved
          </span>
        )}
      </div>
      <div className="flex items-center gap-3">
        {activeTab && activeTab.kind === 'code' && (
          <>
            <span>Ln {cursor.line}, Col {cursor.column}</span>
            <span>{(activeTab.language ?? 'plaintext').toUpperCase()}</span>
          </>
        )}
        <span>UTF-8</span>
        <span>LF</span>
        <span>Rustic v0.3.1</span>
      </div>
    </div>
  );
}
