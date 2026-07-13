import React, { useCallback, useEffect, useRef, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, Download, FileEdit, GitBranch, LogOut, Loader2, MemoryStick, HardDrive, PanelLeftOpen, PanelLeftClose, PanelRightOpen, PanelRightClose, Power, ListTree, Lock, X } from 'lucide-react';
import { IS_WEB } from '@/lib/platform';
import { cn } from '@/lib/utils';
import { GithubIcon } from '@/components/github/icon';
import { IssueQueueDialog } from '@/components/github/issue-queue-dialog';
import { toast } from 'sonner';
import { confirm } from '@/components/confirm-dialog';
import { getActiveEditor, getActiveTab } from '@/lib/active-editor';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { useGit } from '@/state/git';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useEditor } from '@/state/editor';
import { useGithubAuth } from '@/state/github';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuSeparator,
  DropdownMenuLabel,
} from '@/components/ui/dropdown-menu';

const FOCUS_RING = 'rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/60';

function GithubStatusItem() {
  const user = useGithubAuth((s) => s.user);
  const hasToken = useGithubAuth((s) => s.hasToken);
  const loading = useGithubAuth((s) => s.loading);
  const openDialog = useGithubAuth((s) => s.openDialog);
  const signOut = useGithubAuth((s) => s.signOut);
  // Auto issue resolve (web/server build only): quick master toggle + queue
  // viewer, mirrored from the full config in Settings → Agent.
  const [autoResolve, setAutoResolve] = useState(null); // null = not loaded
  const [queueOpen, setQueueOpen] = useState(false);

  useEffect(() => {
    if (!IS_WEB) return;
    invoke('github_auto_get_config')
      .then((r) => setAutoResolve(r.config))
      .catch(() => {});
  }, []);

  const toggleAutoResolve = async (v) => {
    if (!autoResolve) return;
    try {
      const saved = await invoke('github_auto_set_config', {
        enabled: v,
        publicBaseUrl: autoResolve.publicBaseUrl || '',
        label: autoResolve.label || 'rustic',
      });
      setAutoResolve(saved);
    } catch (e) {
      toast.error(String(e));
    }
  };

  // Not signed in — single click opens the sign-in dialog.
  if (!user && !hasToken) {
    return (
      <button
        type="button"
        onClick={openDialog}
        className={cn('flex items-center gap-1 px-1 hover:text-foreground', FOCUS_RING)}
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
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className={cn('flex items-center gap-1 px-1 hover:text-foreground aria-expanded:text-foreground', FOCUS_RING)}
            >
              <GithubIcon className="size-3" />
              <span>{label}</span>
            </button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">Signed in as {label}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="start" side="top" className="min-w-[180px]">
        <DropdownMenuLabel className="flex items-center gap-2">
          <GithubIcon className="size-3.5" />
          <span className="truncate">{label}</span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {IS_WEB && autoResolve && (
          <>
            <div
              className="flex items-center justify-between gap-3 px-2 py-1.5 text-sm"
              onClick={(e) => e.stopPropagation()}
            >
              <span>Auto issue resolve</span>
              <Switch
                checked={!!autoResolve.enabled}
                onCheckedChange={toggleAutoResolve}
                className="scale-75"
              />
            </div>
            <DropdownMenuItem onClick={() => setQueueOpen(true)}>
              <ListTree className="size-3" />
              Issue queue…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
          </>
        )}
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
      {queueOpen && <IssueQueueDialog open={queueOpen} onClose={() => setQueueOpen(false)} />}
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
    // 2s cadence — the backend reads cgroup memory + filesystem usage (both
    // O(1)), so this is cheap and feels live as Chromium/dev-servers spin up.
    const id = setInterval(poll, 2000);
    return () => { active = false; clearInterval(id); };
  }, []);

  if (!usage) return null;

  return (
    <span className="flex items-center gap-3">
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="flex items-center gap-1">
            <MemoryStick className="size-3" />
            {formatBytes(usage.ramProcessBytes)} / {formatBytes(usage.ramTotalBytes)}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">
          RAM in use across the whole VM (server + everything it spawns) / memory limit
        </TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="flex items-center gap-1">
            <HardDrive className="size-3" />
            {formatBytes(usage.diskUsedBytes)} / {formatBytes(usage.diskTotalBytes)}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">Storage used on the data volume / volume capacity</TooltipContent>
      </Tooltip>
    </span>
  );
}

// Web-only task manager: lists every process in the VM and lets the operator
// stop them on the fly. Protected system processes (PID 1, this server + its
// ancestors, kernel threads, core daemons) are flagged red and can't be killed
// — the server enforces that too, so a stray click can't take the box down.
function ProcessManager() {
  const [open, setOpen] = useState(false);
  const [procs, setProcs] = useState([]);
  const [loading, setLoading] = useState(false);
  const [force, setForce] = useState(false);
  const [filter, setFilter] = useState('');
  const [killing, setKilling] = useState({});
  const [err, setErr] = useState('');

  const refresh = useCallback(() => {
    invoke('list_processes')
      .then((rows) => setProcs(Array.isArray(rows) ? rows : []))
      .catch((e) => setErr(`Couldn't load the process list — ${String(e?.message || e)}`))
      .finally(() => setLoading(false));
  }, []);

  // Poll while the dialog is open; stop when it closes.
  useEffect(() => {
    if (!open) return;
    setErr('');
    setLoading(true);
    refresh();
    const id = setInterval(refresh, 2500);
    return () => clearInterval(id);
  }, [open, refresh]);

  const kill = useCallback(
    async (pid) => {
      setKilling((k) => ({ ...k, [pid]: true }));
      setErr('');
      try {
        await invoke('kill_process', { pid, force });
        setTimeout(refresh, 400);
      } catch (e) {
        setErr(`Couldn't end that process — ${String(e?.message || e)}`);
      } finally {
        setKilling((k) => {
          const n = { ...k };
          delete n[pid];
          return n;
        });
      }
    },
    [force, refresh],
  );

  const needle = filter.trim().toLowerCase();
  const shown = needle
    ? procs.filter((p) => `${p.name} ${p.cmd} ${p.pid}`.toLowerCase().includes(needle))
    : procs;

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger asChild>
          <DialogTrigger asChild>
            <button
              type="button"
              className={cn('flex items-center px-1 hover:text-foreground aria-expanded:text-foreground', FOCUS_RING)}
              aria-label="Open task manager"
            >
              <ListTree className="size-3.5" />
            </button>
          </DialogTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">Task Manager — running processes</TooltipContent>
      </Tooltip>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Task Manager</DialogTitle>
          <DialogDescription>
            Processes running in this VM. Protected system processes (shown in red) can&apos;t be
            stopped. Stopping a dev server, build, or agent is safe — your files aren&apos;t touched.
          </DialogDescription>
        </DialogHeader>

        <div className="flex items-center gap-3">
          <Input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter by name, command, or PID…"
            className="h-7 flex-1 text-xs"
          />
          <label
            className="flex shrink-0 items-center gap-1.5 whitespace-nowrap text-[12px] text-muted-foreground"
            title="Uses SIGKILL — the process is ended immediately with no chance to clean up"
          >
            <Switch checked={force} onCheckedChange={setForce} />
            End immediately (force)
          </label>
        </div>

        {err && <div className="text-[12px] text-destructive">{err}</div>}

        <div className="max-h-[55vh] overflow-auto rounded-md border border-border/50">
          <table className="w-full text-[12px]">
            <thead className="sticky top-0 z-10 bg-popover text-muted-foreground">
              <tr className="border-b border-border/50">
                <th className="px-2 py-1.5 text-left font-medium">Name</th>
                <th className="px-2 py-1.5 text-right font-medium">PID</th>
                <th className="px-2 py-1.5 text-right font-medium">CPU</th>
                <th className="px-2 py-1.5 text-right font-medium">Memory</th>
                <th className="w-8 px-2 py-1.5" />
              </tr>
            </thead>
            <tbody>
              {shown.map((p) => (
                <tr
                  key={p.pid}
                  className={cn('border-b border-border/20', p.protected && 'text-destructive')}
                >
                  <td className="max-w-[300px] px-2 py-1" title={p.cmd}>
                    <span className="flex items-center gap-1 truncate">
                      {p.protected && <Lock className="size-3 shrink-0" />}
                      <span className="truncate">{p.name}</span>
                    </span>
                  </td>
                  <td className="px-2 py-1 text-right tabular-nums">{p.pid}</td>
                  <td className="px-2 py-1 text-right tabular-nums">
                    {Number(p.cpuPercent ?? 0).toFixed(1)}%
                  </td>
                  <td className="px-2 py-1 text-right tabular-nums">{formatBytes(p.memoryBytes)}</td>
                  <td className="px-2 py-1 text-right">
                    {p.protected ? (
                      <span className="text-[10px] uppercase tracking-wide opacity-60">locked</span>
                    ) : (
                      <button
                        type="button"
                        onClick={() => kill(p.pid)}
                        disabled={!!killing[p.pid]}
                        className="rounded p-0.5 text-muted-foreground hover:text-destructive disabled:opacity-50"
                        title={force ? 'End immediately (SIGKILL)' : 'End process (SIGTERM)'}
                      >
                        {killing[p.pid] ? (
                          <Loader2 className="size-3.5 animate-spin" />
                        ) : (
                          <X className="size-3.5" />
                        )}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
              {!shown.length && (
                <tr>
                  <td colSpan={5} className="px-2 py-4 text-center text-muted-foreground">
                    {loading ? 'Loading…' : 'No processes'}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </DialogContent>
    </Dialog>
  );
}

// Status-bar button that pins the left activity-bar island open/closed. The
// island otherwise only reveals on hover of the screen's left edge, which is
// impossible on a touch device — this gives a no-mouse way in.
function IslandToggle() {
  const islandOpen = useLayout((s) => s.islandOpen);
  const toggleIsland = useLayout((s) => s.toggleIsland);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={toggleIsland}
          aria-pressed={islandOpen}
          className={cn('flex items-center gap-1 px-1 hover:text-foreground aria-pressed:text-foreground', FOCUS_RING)}
        >
          {islandOpen ? <PanelLeftClose className="size-3.5" /> : <PanelLeftOpen className="size-3.5" />}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">{islandOpen ? 'Unpin activity bar' : 'Pin activity bar'}</TooltipContent>
    </Tooltip>
  );
}

// Status-bar button that pins the right dock island open/closed. Mirrors
// IslandToggle for the right edge — the island otherwise only reveals on
// hover, which is impossible on a touch device.
function RightIslandToggle() {
  const rightIslandOpen = useLayout((s) => s.rightIslandOpen);
  const toggleRightIsland = useLayout((s) => s.toggleRightIsland);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={toggleRightIsland}
          aria-pressed={rightIslandOpen}
          className={cn('flex items-center gap-1 px-1 hover:text-foreground aria-pressed:text-foreground', FOCUS_RING)}
        >
          {rightIslandOpen ? <PanelRightClose className="size-3.5" /> : <PanelRightOpen className="size-3.5" />}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">{rightIslandOpen ? 'Unpin dock' : 'Pin dock'}</TooltipContent>
    </Tooltip>
  );
}

// localStorage key the web transport stores the session token under (mirrors
// transport-core.js TOKEN_KEY). Cleared on power-off so the next request 401s
// and re-prompts for the password.
const SESSION_TOKEN_KEY = 'rustic_session_token';

// Web-only "power button": flush every background process the server spun up
// (terminals + their dev servers, agents, MCP, the browser, tunnels) and log
// out. Also owns the idle auto-logout timer, which fires the same flush after a
// configurable period of inactivity unless "keep alive" is on.
function PowerButton() {
  const [busy, setBusy] = useState(false);

  // The actual power-off: ask the server to flush + invalidate the session,
  // then drop our token and reload (which lands on the login overlay). Runs
  // even if the request errors — we still want to force a re-login.
  const doPowerOff = useCallback(async () => {
    setBusy(true);
    try {
      await invoke('power_off');
    } catch (e) {
      console.error('[power] power_off failed', e);
    } finally {
      try {
        localStorage.removeItem(SESSION_TOKEN_KEY);
      } catch {
        /* private mode — ignore */
      }
      location.reload();
    }
  }, []);

  const onClick = useCallback(async () => {
    if (busy) return;
    const ok = await confirm({
      title: 'Power off?',
      description:
        'This ends all terminals, dev servers, agents, the browser and tunnels, ' +
        'and logs you out. Your files are safe.',
      confirmLabel: 'Power off',
      destructive: true,
    });
    if (ok === true) doPowerOff();
  }, [busy, doPowerOff]);

  // Idle auto-logout. Re-arms on any user activity; disabled when keepAlive is
  // on. Reads config on mount and re-applies live when Settings broadcasts a
  // change (rustic:power-config-changed).
  const timerRef = useRef(null);
  useEffect(() => {
    const cfg = { keepAlive: false, idleTimeoutMinutes: 10 };
    let disposed = false;

    const clearTimer = () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
    const arm = () => {
      clearTimer();
      if (cfg.keepAlive) return;
      const ms = Math.max(1, cfg.idleTimeoutMinutes) * 60 * 1000;
      timerRef.current = setTimeout(() => {
        if (!disposed) doPowerOff();
      }, ms);
    };
    const onActivity = () => arm();
    const applyConfig = (c) => {
      cfg.keepAlive = !!c.keepAlive;
      cfg.idleTimeoutMinutes = c.idleTimeoutMinutes || 10;
      arm();
    };

    invoke('get_power_config')
      .then((c) => { if (!disposed && c) applyConfig(c); })
      .catch(() => {});

    const events = ['mousemove', 'mousedown', 'keydown', 'wheel', 'touchstart', 'scroll'];
    events.forEach((e) => window.addEventListener(e, onActivity, { passive: true }));
    const onCfgChange = (ev) => { if (ev.detail) applyConfig(ev.detail); };
    window.addEventListener('rustic:power-config-changed', onCfgChange);

    return () => {
      disposed = true;
      clearTimer();
      events.forEach((e) => window.removeEventListener(e, onActivity));
      window.removeEventListener('rustic:power-config-changed', onCfgChange);
    };
  }, [doPowerOff]);

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onClick}
          disabled={busy}
          className={cn('flex items-center px-1 hover:text-destructive disabled:opacity-50', FOCUS_RING)}
          aria-label="Power off and log out"
        >
          {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Power className="size-3.5" />}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">Power off — flush all processes and log out</TooltipContent>
    </Tooltip>
  );
}

function VersionUpdater() {
  /** Shows the app version; on desktop, auto-checks GitHub for updates and turns into a one-click download → install → relaunch flow. */
  const [version, setVersion] = useState('');
  const [phase, setPhase] = useState('idle');
  const [availableVersion, setAvailableVersion] = useState('');
  const [progress, setProgress] = useState(0);
  const updateRef = useRef(null);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const runCheck = useCallback(async (silent) => {
    if (IS_WEB) return;
    setPhase('checking');
    try {
      const { check } = await import('@tauri-apps/plugin-updater');
      const update = await check();
      if (update) {
        updateRef.current = update;
        setAvailableVersion(update.version);
        setPhase('available');
        if (silent) {
          toast.info(`Rustic v${update.version} is available`, {
            description: 'Click "Update available" in the status bar to install.',
          });
        }
      } else {
        updateRef.current = null;
        setPhase('idle');
        if (!silent) toast.success("You're on the latest version");
      }
    } catch (e) {
      updateRef.current = null;
      setPhase('idle');
      if (!silent) toast.error('Update check failed', { description: String(e) });
    }
  }, []);

  useEffect(() => {
    if (IS_WEB) return;
    // Delay the startup check so it never competes with app boot work.
    const t = setTimeout(() => runCheck(true), 5000);
    return () => clearTimeout(t);
  }, [runCheck]);

  const installUpdate = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;
    const ok = await confirm({
      title: `Update to Rustic v${update.version}?`,
      description: 'The update will download and install, then Rustic will restart automatically. Unsaved changes will be lost.',
      confirmLabel: 'Update & restart',
    });
    if (!ok) return;
    setPhase('downloading');
    setProgress(0);
    try {
      let total = 0;
      let received = 0;
      await update.downloadAndInstall((ev) => {
        if (ev.event === 'Started') {
          total = ev.data.contentLength ?? 0;
        } else if (ev.event === 'Progress') {
          received += ev.data.chunkLength;
          if (total > 0) setProgress(Math.min(99, Math.round((received / total) * 100)));
        } else if (ev.event === 'Finished') {
          setProgress(100);
        }
      });
      setPhase('installing');
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    } catch (e) {
      setPhase('available');
      toast.error('Update failed', { description: String(e) });
    }
  }, []);

  if (IS_WEB) return <span>Rustic{version ? ` v${version}` : ''}</span>;

  if (phase === 'downloading') {
    return (
      <span className="flex items-center gap-1 text-foreground tabular-nums">
        <Loader2 className="size-3 animate-spin" />
        Updating… {progress}%
      </span>
    );
  }
  if (phase === 'installing') {
    return (
      <span className="flex items-center gap-1 text-foreground">
        <Loader2 className="size-3 animate-spin" />
        Restarting…
      </span>
    );
  }
  if (phase === 'available') {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={installUpdate}
            className={cn('flex items-center gap-1 px-1 font-medium text-primary hover:underline', FOCUS_RING)}
          >
            <Download className="size-3" />
            Update available
          </button>
        </TooltipTrigger>
        <TooltipContent side="top">Rustic v{availableVersion} — click to download, install and restart</TooltipContent>
      </Tooltip>
    );
  }
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={() => runCheck(false)}
          disabled={phase === 'checking'}
          className={cn('flex items-center gap-1 px-1 hover:text-foreground disabled:opacity-70', FOCUS_RING)}
        >
          {phase === 'checking' && <Loader2 className="size-3 animate-spin" />}
          Rustic{version ? ` v${version}` : ''}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">{phase === 'checking' ? 'Checking for updates…' : 'Check for updates'}</TooltipContent>
    </Tooltip>
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

  const openScm = useCallback(() => {
    const s = useLayout.getState();
    // setActiveSidebarPanel toggles visibility when the panel is already
    // active — for a status-bar shortcut we only ever want to open, never hide.
    if (!(s.activeSidebarPanel === SIDEBAR_PANELS.SCM && s.sidebarVisible)) {
      s.setActiveSidebarPanel(SIDEBAR_PANELS.SCM);
    }
  }, []);

  const goToLine = useCallback(() => {
    const editor = getActiveEditor();
    if (!editor) return;
    editor.focus();
    editor.getAction('editor.action.gotoLine')?.run();
  }, []);

  let eol = null;
  if (activeTab && activeTab.kind === 'code') {
    const editor = getActiveEditor();
    // The registered editor can lag a tab switch — only trust its model when
    // it belongs to the tab the status bar is describing.
    if (editor && getActiveTab()?.id === activeTab.id) {
      try {
        const model = editor.getModel();
        if (model) eol = model.getEOL() === '\r\n' ? 'CRLF' : 'LF';
      } catch { /* disposed model */ }
    }
  }

  const ahead = projectGit?.aheadBehind?.ahead ?? 0;
  const behind = projectGit?.aheadBehind?.behind ?? 0;

  return (
    <div className="flex h-6 shrink-0 items-center justify-between border-t border-border bg-background px-2 text-[11px] text-muted-foreground select-none">
      <div className="flex items-center gap-3">
        {islandToggle && <IslandToggle />}
        {projectGit?.currentBranch && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={openScm}
                className={cn('flex items-center gap-1 px-1 hover:text-foreground', FOCUS_RING)}
              >
                <GitBranch className="size-3" />
                <span className="max-w-40 truncate">{projectGit.currentBranch}</span>
                {(ahead > 0 || behind > 0) && (
                  <span className="tabular-nums">
                    {ahead > 0 ? `${ahead}↑` : ''}{behind > 0 ? `${behind}↓` : ''}
                  </span>
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="top">Open Source Control</TooltipContent>
          </Tooltip>
        )}
        <GithubStatusItem />
        {IS_WEB && <ResourceMonitor />}
        {IS_WEB && <ProcessManager />}
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
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  onClick={goToLine}
                  className={cn('px-1 hover:text-foreground', FOCUS_RING)}
                >
                  Ln {cursor.line}, Col {cursor.column}
                </button>
              </TooltipTrigger>
              <TooltipContent side="top">Go to line…</TooltipContent>
            </Tooltip>
            <span>{(activeTab.language ?? 'plaintext').toUpperCase()}</span>
            {eol && <span>{eol}</span>}
          </>
        )}
        <VersionUpdater />
        {IS_WEB && <PowerButton />}
        {islandToggle && <RightIslandToggle />}
      </div>
    </div>
  );
}
