// Top-bar transfers tray: an icon with an animated progress ring while
// anything is moving; click for running + finished transfers with size,
// speed, ETA, cancel, open location and clear.
import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowDownToLine, ArrowUpFromLine, FolderOpen, X, Trash2, ArrowDownUp, Loader2, Pause, Play, PauseCircle } from 'lucide-react';
import { toast } from 'sonner';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { formatBytes, formatEta, formatSpeed } from '@/lib/transfer-format';
import { openLocation } from '@/lib/open-location';

const RUNNING = new Set(['running', 'waiting']);

/** Merge one `rustic:transfer` payload into the list. */
function upsert(list, t) {
  const i = list.findIndex((x) => x.id === t.id);
  if (i === -1) return [...list, t];
  const next = list.slice();
  next[i] = t;
  return next;
}

/** Small circular progress ring (indeterminate spin when total unknown). */
function Ring({ pct, size = 18, paused = false }) {
  const r = (size - 3) / 2;
  const c = 2 * Math.PI * r;
  if (pct == null) {
    return paused
      ? <PauseCircle className="text-warning" style={{ width: size, height: size }} />
      : <Loader2 className="animate-spin text-primary" style={{ width: size, height: size }} />;
  }
  return (
    <svg width={size} height={size} className="-rotate-90">
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" strokeWidth="2.5" className="stroke-muted-foreground/25" />
      <circle
        cx={size / 2} cy={size / 2} r={r} fill="none" strokeWidth="2.5" strokeLinecap="round"
        className={`${paused ? 'stroke-warning' : 'stroke-primary'} transition-[stroke-dashoffset] duration-300`}
        strokeDasharray={c} strokeDashoffset={c * (1 - Math.min(1, pct))}
      />
    </svg>
  );
}

/** One transfer row. */
function TransferRow({ t }) {
  const running = RUNNING.has(t.state);
  const pct = t.total > 0 ? Math.min(1, t.done / t.total) : null;
  const Icon = t.direction === 'pull' ? ArrowDownToLine : ArrowUpFromLine;
  const stateText = {
    waiting: t.detail || 'Waiting for approval…',
    done: 'Done',
    failed: t.error || 'Failed',
    cancelled: 'Cancelled',
  }[t.state];
  const cancel = () => invoke('lan_transfer_cancel', { id: t.id }).catch((e) => toast.error(String(e?.message || e)));
  const pause = () => invoke('lan_transfer_pause', { id: t.id }).catch((e) => toast.error(String(e?.message || e)));
  const resume = () => invoke('lan_transfer_resume', { id: t.id }).catch((e) => toast.error(String(e?.message || e)));
  const clear = () => invoke('lan_transfer_clear', { id: t.id }).catch(() => {});
  const userPaused = t.paused && t.paused_reason === 'Paused';
  return (
    <div className="group space-y-1.5 border-b border-border/40 px-3 py-2.5 last:border-b-0">
      <div className="flex items-center gap-2 text-[12px]">
        <Icon className={`h-3.5 w-3.5 shrink-0 ${t.direction === 'pull' ? 'text-success' : 'text-info'}`} />
        <span className="min-w-0 flex-1 truncate font-medium" title={t.label}>{t.label}</span>
        <span className="shrink-0 text-[10.5px] text-muted-foreground">{t.direction === 'pull' ? 'from' : 'to'} {t.peer}</span>
      </div>
      {running && t.state === 'running' && (
        <div className="h-1 overflow-hidden rounded-full bg-muted">
          {pct == null
            ? <div className={`h-full w-1/3 rounded-full ${t.paused ? 'bg-warning/70' : 'animate-[transfer-indeterminate_1.2s_ease-in-out_infinite] bg-primary'}`} />
            : <div className={`h-full rounded-full transition-[width] duration-300 ${t.paused ? 'bg-warning/70' : 'bg-primary'}`} style={{ width: `${(pct * 100).toFixed(1)}%` }} />}
        </div>
      )}
      <div className="flex items-center gap-2 text-[10.5px] text-muted-foreground">
        {t.state === 'running' && t.paused ? (
          <span className="flex min-w-0 flex-1 items-center gap-1 truncate text-warning" title={t.paused_reason || 'Paused'}>
            <PauseCircle className="size-3 shrink-0" />
            <span className="truncate">{t.paused_reason || 'Paused'}</span>
            <span className="shrink-0 tabular-nums text-muted-foreground">· {formatBytes(t.done)}{t.total > 0 ? ` / ${formatBytes(t.total)}` : ''}</span>
          </span>
        ) : t.state === 'running' ? (
          <span className="min-w-0 flex-1 truncate tabular-nums">
            {formatBytes(t.done)}{t.total > 0 ? ` / ${formatBytes(t.total)}` : ''}
            {t.speed_bps > 0 ? ` · ${formatSpeed(t.speed_bps)}` : ''}
            {t.eta_secs != null ? ` · ${formatEta(t.eta_secs)} left` : ''}
            {t.phase === 'cancelling' ? ' · cancelling…' : ''}
          </span>
        ) : (
          <span className={`min-w-0 flex-1 truncate ${t.state === 'failed' ? 'text-destructive' : ''}`} title={stateText}>
            {stateText}{t.state === 'done' && t.total > 0 ? ` · ${formatBytes(t.total)}` : ''}
          </span>
        )}
        {running ? (
          <>
            {t.can_pause && t.state === 'running' && (
              userPaused ? (
                <button type="button" title="Resume" onClick={resume} className="rounded p-1 hover:bg-accent hover:text-foreground">
                  <Play className="h-3.5 w-3.5" />
                </button>
              ) : (
                <button type="button" title={t.paused ? 'Pause (it will stay paused when the connection returns)' : 'Pause'} onClick={pause} className="rounded p-1 hover:bg-accent hover:text-foreground">
                  <Pause className="h-3.5 w-3.5" />
                </button>
              )
            )}
            <button type="button" onClick={cancel} className="rounded px-1.5 py-0.5 hover:bg-destructive/15 hover:text-destructive">
              Cancel
            </button>
          </>
        ) : (
          <>
            {t.location && (
              <button type="button" title="Open location" onClick={() => openLocation(t.location)} className="rounded p-1 hover:bg-accent hover:text-foreground">
                <FolderOpen className="h-3.5 w-3.5" />
              </button>
            )}
            <button type="button" title="Clear" onClick={clear} className="rounded p-1 hover:bg-accent hover:text-foreground">
              <X className="h-3.5 w-3.5" />
            </button>
          </>
        )}
      </div>
    </div>
  );
}

/** Mounted in the top bar; renders nothing until the first transfer. */
export function TransfersTray() {
  const [list, setList] = useState([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    invoke('lan_transfers').then((l) => { if (!disposed && Array.isArray(l)) setList(l); }).catch(() => {});
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('rustic:transfer', (e) => {
        const t = e.payload;
        if (!t?.id) return;
        setList((l) => upsert(l, t));
        if (t.state === 'failed' && t.error && t.error !== 'Cancelled') toast.error(`${t.label}: ${t.error}`);
      }))
      .then((fn) => { if (disposed) fn(); else unlisten = fn; })
      .catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // Cleared entries disappear on the backend; refresh when the popover opens.
  useEffect(() => {
    if (open) invoke('lan_transfers').then((l) => Array.isArray(l) && setList(l)).catch(() => {});
  }, [open]);

  const running = useMemo(() => list.filter((t) => RUNNING.has(t.state)), [list]);
  const aggregate = useMemo(() => {
    const known = running.filter((t) => t.state === 'running' && t.total > 0);
    if (!known.length) return null;
    const done = known.reduce((s, t) => s + Math.min(t.done, t.total), 0);
    const total = known.reduce((s, t) => s + t.total, 0);
    return total ? done / total : null;
  }, [running]);

  if (!list.length) return null;
  const sorted = [...running, ...list.filter((t) => !RUNNING.has(t.state)).reverse()];
  const clearAll = () => invoke('lan_transfer_clear', {}).then(() => invoke('lan_transfers')).then((l) => Array.isArray(l) && setList(l)).catch(() => {});

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          title={running.length ? `${running.length} transfer${running.length === 1 ? '' : 's'} running` : 'Transfers'}
          className="relative flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          {running.length ? <Ring pct={aggregate} paused={running.every((t) => t.paused)} /> : <ArrowDownUp className="h-4 w-4" />}
          {running.length > 1 && (
            <span className="absolute -right-0.5 -top-0.5 rounded-full bg-primary px-1 text-[9px] font-semibold leading-[14px] text-primary-foreground">
              {running.length}
            </span>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[360px] p-0">
        <div className="flex items-center justify-between border-b border-border/50 px-3 py-2">
          <span className="text-[12px] font-semibold">Transfers</span>
          {list.some((t) => !RUNNING.has(t.state)) && (
            <button type="button" onClick={clearAll} className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10.5px] text-muted-foreground hover:bg-accent hover:text-foreground">
              <Trash2 className="h-3 w-3" /> Clear finished
            </button>
          )}
        </div>
        <div className="max-h-[420px] overflow-y-auto">
          {sorted.map((t) => <TransferRow key={t.id} t={t} />)}
        </div>
      </PopoverContent>
    </Popover>
  );
}

export default TransfersTray;
