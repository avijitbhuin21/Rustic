import React, { useEffect, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, FileEdit, LogOut, Loader2, MemoryStick, HardDrive, PanelLeftOpen, PanelLeftClose } from 'lucide-react';
import { IS_WEB } from '@/lib/platform';
import { GithubIcon } from '@/components/github/icon';
import { useGit } from '@/state/git';
import { useLayout } from '@/state/layout';
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

/// Formats a byte count into a compact human-readable string (e.g. "512 MB").
function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  const digits = value >= 100 || i <= 1 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[i]}`;
}

/// Web-only status-bar widget polling the server's process RAM + data-volume
/// disk usage every 5 seconds.
function ResourceMonitor() {
  const [usage, setUsage] = useState(null);

  useEffect(() => {
    let active = true;
    const poll = () => {
      invoke('get_resource_usage')
        .then((u) => { if (active) setUsage(u); })
        .catch(() => {});
    };
    poll();
    const id = setInterval(poll, 5000);
    return () => { active = false; clearInterval(id); };
  }, []);

  if (!usage) return null;

  return (
    <span className="flex items-center gap-3" title="Resource Monitor">
      <span className="flex items-center gap-1" title="RAM used by Rustic">
        <MemoryStick className="size-3" />
        {formatBytes(usage.ramProcessBytes)} / {formatBytes(usage.ramTotalBytes)}
      </span>
      <span className="flex items-center gap-1" title="Storage used by the Rustic data folder / volume capacity">
        <HardDrive className="size-3" />
        {formatBytes(usage.diskUsedBytes)} / {formatBytes(usage.diskTotalBytes)}
      </span>
    </span>
  );
}

// Status-bar button that pins the left activity-bar island open/closed. The
// island otherwise only reveals on hover of the screen's left edge, which is
// impossible on a touch device — this gives a no-mouse way in. Only rendered
// in the desktop web layout (where the island exists); see App.jsx.
function IslandToggle() {
  const islandOpen = useLayout((s) => s.islandOpen);
  const toggleIsland = useLayout((s) => s.toggleIsland);
  return (
    <button
      type="button"
      onClick={toggleIsland}
      aria-pressed={islandOpen}
      className="flex items-center gap-1 px-1 hover:text-foreground aria-pressed:text-foreground"
      title={islandOpen ? 'Hide activity bar' : 'Show activity bar'}
    >
      {islandOpen ? <PanelLeftClose className="size-3.5" /> : <PanelLeftOpen className="size-3.5" />}
    </button>
  );
}

export function StatusBar({ islandToggle = false }) {
  const projectGit = useGit((s) => s.projects[s.activeProjectId]);
  const groups      = useEditor((s) => s.groups);
  const activeGroupId = useEditor((s) => s.activeGroupId);
  const cursor      = useEditor((s) => s.cursor);

  const allTabs   = (groups ?? []).flatMap((g) => g.tabs);
  const dirtyCount = allTabs.filter((t) => t.dirty).length;
  const activeGroup = groups.find((g) => g.id === activeGroupId);
  const activeTab   = activeGroup?.tabs.find((t) => t.id === activeGroup.activeId) ?? null;
  const conflicts = projectGit?.conflicts?.length ?? 0;

  const [version, setVersion] = useState('');
  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  return (
    <div className="flex h-6 shrink-0 items-center justify-between border-t border-border bg-background px-2 text-[11px] text-muted-foreground select-none">
      <div className="flex items-center gap-3">
        {islandToggle && <IslandToggle />}
        <GithubStatusItem />
        {IS_WEB && <ResourceMonitor />}
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
        <span>Rustic{version ? ` v${version}` : ''}</span>
      </div>
    </div>
  );
}
