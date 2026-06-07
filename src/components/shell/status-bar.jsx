import React, { useCallback, useEffect, useRef, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, FileEdit, LogOut, Loader2, MemoryStick, HardDrive, PanelLeftOpen, PanelLeftClose, Power, ListTree, Lock, X } from 'lucide-react';
import { IS_WEB } from '@/lib/platform';
import { cn } from '@/lib/utils';
import { GithubIcon } from '@/components/github/icon';
import { useGit } from '@/state/git';
import { useLayout } from '@/state/layout';
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
    // 2s cadence — the backend reads cgroup memory + filesystem usage (both
    // O(1)), so this is cheap and feels live as Chromium/dev-servers spin up.
    const id = setInterval(poll, 2000);
    return () => { active = false; clearInterval(id); };
  }, []);

  if (!usage) return null;

  return (
    <span className="flex items-center gap-3" title="Resource Monitor">
      <span className="flex items-center gap-1" title="RAM in use across the whole VM (server + everything it spawns) / memory limit">
        <MemoryStick className="size-3" />
        {formatBytes(usage.ramProcessBytes)} / {formatBytes(usage.ramTotalBytes)}
      </span>
      <span className="flex items-center gap-1" title="Storage used on the data volume / volume capacity">
        <HardDrive className="size-3" />
        {formatBytes(usage.diskUsedBytes)} / {formatBytes(usage.diskTotalBytes)}
      </span>
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
      .catch((e) => setErr(String(e?.message || e)))
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
        setErr(String(e?.message || e));
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
      <DialogTrigger asChild>
        <button
          type="button"
          className="flex items-center px-1 hover:text-foreground aria-expanded:text-foreground"
          title="Task Manager — running processes"
          aria-label="Open task manager"
        >
          <ListTree className="size-3.5" />
        </button>
      </DialogTrigger>
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
          <label className="flex shrink-0 items-center gap-1.5 whitespace-nowrap text-[12px] text-muted-foreground">
            <Switch checked={force} onCheckedChange={setForce} />
            Force (SIGKILL)
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
                        title={force ? 'Force kill (SIGKILL)' : 'Stop (SIGTERM)'}
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

  const onClick = useCallback(() => {
    if (busy) return;
    const ok = window.confirm(
      'Power off?\n\nThis ends all terminals, dev servers, agents, the browser and tunnels, ' +
        'and logs you out. Your files are safe.',
    );
    if (ok) doPowerOff();
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
    <button
      type="button"
      onClick={onClick}
      disabled={busy}
      className="flex items-center px-1 hover:text-destructive disabled:opacity-50"
      title="Power off — flush all processes and log out"
      aria-label="Power off and log out"
    >
      {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Power className="size-3.5" />}
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
            <span>Ln {cursor.line}, Col {cursor.column}</span>
            <span>{(activeTab.language ?? 'plaintext').toUpperCase()}</span>
          </>
        )}
        <span>UTF-8</span>
        <span>LF</span>
        <span>Rustic{version ? ` v${version}` : ''}</span>
        {IS_WEB && <PowerButton />}
      </div>
    </div>
  );
}
