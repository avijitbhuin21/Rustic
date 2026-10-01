// Cloud & Sync building blocks: saved remote backends, segmented sliders,
// searchable multi-select item lists, machine rows, and Add machine.
import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Loader2, Check, X, Plus, Search, Wifi, Globe, Server } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';

const BACKENDS_KEY = 'rustic.remoteBackends';
const LEGACY_URL_KEY = 'rustic.remoteBackend.url';

/** Hostname of a URL, for default backend names. */
export function hostOf(url) {
  try { return new URL(url).host || url; } catch { return url; }
}

/** Read the saved remote backends, migrating the old single-URL setting once. */
function loadBackends() {
  try {
    const raw = localStorage.getItem(BACKENDS_KEY);
    if (raw) {
      const list = JSON.parse(raw);
      if (Array.isArray(list)) return list.filter((b) => b && b.url);
    }
    const legacy = localStorage.getItem(LEGACY_URL_KEY);
    if (legacy && legacy.trim()) {
      return [{ id: `rb-${Date.now()}`, name: hostOf(legacy), url: legacy.trim().replace(/\/+$/, '') }];
    }
  } catch {}
  return [];
}

/** Saved remote backends, persisted to localStorage. The first one also feeds the explorer's per-project sync. */
export function useSavedBackends() {
  const [backends, setBackends] = useState(loadBackends);
  useEffect(() => {
    try {
      localStorage.setItem(BACKENDS_KEY, JSON.stringify(backends));
      if (backends[0]) localStorage.setItem(LEGACY_URL_KEY, backends[0].url);
      else localStorage.removeItem(LEGACY_URL_KEY);
    } catch {}
  }, [backends]);
  return [backends, setBackends];
}

/** Copy text to the clipboard with a toast. */
export async function copyText(text, label = 'Copied') {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(label);
  } catch {
    toast.error('Could not copy');
  }
}

/** Inline rename field. */
export function InlineRename({ value, placeholder, onSave, onCancel }) {
  const [text, setText] = useState(value || '');
  return (
    <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
      <Input
        autoFocus
        value={text}
        placeholder={placeholder}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onSave(text.trim());
          if (e.key === 'Escape') onCancel();
        }}
        className="h-7 w-48 text-[12px]"
      />
      <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => onSave(text.trim())}><Check className="size-3.5" /></Button>
      <Button size="icon-sm" variant="ghost" className="size-7" onClick={onCancel}><X className="size-3.5" /></Button>
    </div>
  );
}

/** Sliding segmented control (two or three options) with optional counts. */
export function Slider({ value, onChange, options, className }) {
  const idx = Math.max(0, options.findIndex((o) => o.id === value));
  const n = options.length;
  return (
    <div className={cn('relative grid rounded-lg border border-border/60 bg-muted/30 p-0.5', className)} style={{ gridTemplateColumns: `repeat(${n}, minmax(0, 1fr))` }}>
      <span
        aria-hidden
        className="absolute inset-y-0.5 left-0.5 rounded-md bg-background shadow-sm transition-transform duration-200 ease-out"
        style={{ width: `calc(${100 / n}% - ${4 / n}px)`, transform: `translateX(${idx * 100}%)` }}
      />
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          onClick={() => onChange(o.id)}
          className={cn(
            'relative z-10 flex items-center justify-center gap-1.5 truncate rounded-md px-3 py-1.5 text-[12px] transition-colors',
            value === o.id ? 'text-foreground' : 'text-muted-foreground hover:text-foreground',
          )}
        >
          {o.icon && <o.icon className="size-3.5" />}
          {o.label}
          {o.count != null && (
            <span className="rounded-full bg-muted px-1.5 text-[10px] tabular-nums text-muted-foreground">{o.count}</span>
          )}
        </button>
      ))}
    </div>
  );
}

/** Search box that filters as you type. */
export function SearchBox({ value, onChange, placeholder, className }) {
  return (
    <div className={cn('relative', className)}>
      <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
      <Input value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} className="h-8 pl-8 text-[12px]" />
    </div>
  );
}

/**
 * Searchable multi-select list (projects or metadata items). `items` are
 * `{ id, label, sub?, badge?, badgeTone? }`; `selected` is a Set of ids.
 */
export function ItemPicker({ title, items, selected, onChange, loading, empty, placeholder }) {
  const [q, setQ] = useState('');
  const visible = useMemo(() => {
    const s = q.trim().toLowerCase();
    if (!s) return items;
    return items.filter((i) => `${i.label} ${i.sub || ''}`.toLowerCase().includes(s));
  }, [items, q]);
  const allOn = visible.length > 0 && visible.every((i) => selected.has(i.id));
  const toggleAll = () => {
    const next = new Set(selected);
    visible.forEach((i) => (allOn ? next.delete(i.id) : next.add(i.id)));
    onChange(next);
  };
  const toggle = (id) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id); else next.add(id);
    onChange(next);
  };
  return (
    <div className="flex min-h-0 min-w-0 flex-col gap-1.5">
      <div className="flex items-center gap-2 px-0.5">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70">{title}</span>
        <span className="text-[10.5px] tabular-nums text-muted-foreground">{selected.size}/{items.length}</span>
        {visible.length > 0 && (
          <button type="button" className="ml-auto text-[11px] text-muted-foreground hover:text-foreground" onClick={toggleAll}>
            {allOn ? 'Clear' : 'Select all'}
          </button>
        )}
      </div>
      <SearchBox value={q} onChange={setQ} placeholder={placeholder || `Search ${title.toLowerCase()}…`} />
      <div className="h-56 overflow-y-auto rounded-xl border border-border/50 bg-muted/20">
        {loading ? (
          <div className="flex items-center gap-1.5 p-3 text-[11.5px] text-muted-foreground"><Loader2 className="size-3 animate-spin" /> Loading…</div>
        ) : visible.length === 0 ? (
          <div className="p-3 text-[11.5px] text-muted-foreground">{q ? 'No matches.' : empty}</div>
        ) : (
          <ul className="divide-y divide-border/40">
            {visible.map((i) => (
              <li key={i.id}>
                <label className="flex cursor-pointer items-center gap-2.5 px-3 py-2 text-[12px] hover:bg-muted/40">
                  <input type="checkbox" className="accent-primary" checked={selected.has(i.id)} onChange={() => toggle(i.id)} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate">{i.label}</div>
                    {i.sub && <div className="truncate font-mono text-[10.5px] text-muted-foreground">{i.sub}</div>}
                  </div>
                  {i.badge && <span className={cn('shrink-0 text-[10px]', i.badgeTone || 'text-muted-foreground')}>{i.badge}</span>}
                </label>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

/** Status dot + label. */
export function StatusPill({ online }) {
  const label = online == null ? 'checking' : online ? 'online' : 'offline';
  return (
    <span className={cn('flex items-center gap-1 text-[10.5px]', online ? 'text-emerald-500' : 'text-muted-foreground')}>
      <span className={cn('size-1.5 rounded-full', online ? 'bg-emerald-500' : online === false ? 'bg-muted-foreground/50' : 'bg-amber-500 animate-pulse')} />
      {label}
    </span>
  );
}

/**
 * One machine card in a connected list. Remote backends get a tint from the
 * theme's primary colour so they stand apart from sync-only desktops.
 */
export function MachineCard({ machine, selected, onClick, actions, children }) {
  const backend = machine.kind === 'remote';
  const Icon = backend ? Server : machine.viaInternet ? Globe : Wifi;
  return (
    <div className={cn(backend ? 'bg-primary/[0.06]' : '', selected && (backend ? 'bg-primary/[0.12]' : 'bg-muted/50'))}>
      <div
        role="button"
        tabIndex={0}
        onClick={onClick}
        onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && onClick?.()}
        className={cn('flex cursor-pointer items-center gap-3 px-3 py-2.5 hover:bg-muted/40', backend && 'hover:bg-primary/10')}
      >
        <span className={cn('flex size-7 shrink-0 items-center justify-center rounded-lg', backend ? 'bg-primary/15 text-primary' : 'bg-muted text-muted-foreground')}>
          <Icon className="size-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 text-[12.5px]">
            <span className="truncate font-medium">{machine.name}</span>
            <StatusPill online={machine.online} />
            {backend && <span className="rounded border border-primary/40 px-1 text-[9.5px] text-primary">backend</span>}
            {machine.kind === 'lan' && !machine.paired && <span className="text-[10px] text-amber-500">not paired</span>}
            {machine.windowOpen && <span className="text-[10px] text-emerald-500">window open</span>}
          </div>
          {machine.subtitle && <div className="truncate font-mono text-[10.5px] text-muted-foreground">{machine.subtitle}</div>}
        </div>
        <div className="flex shrink-0 items-center gap-0.5" onClick={(e) => e.stopPropagation()}>{actions}</div>
      </div>
      {children}
    </div>
  );
}

/**
 * Add machine. `mode` 'sync' adds a desktop to sync with — on the local
 * network (IP[:port]) or over the internet (port-forwarded public IP:port or
 * the other machine's Cloudflare tunnel URL). `mode` 'backend' saves a
 * remote rustic-server (URL + password).
 */
export function AddMachineDialog({ open, mode, onClose, lanEnabled, onLanAdded, onBackendAdded }) {
  const [tab, setTab] = useState('lan');
  const [address, setAddress] = useState('');
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!open) return;
    setTab('lan'); setAddress(''); setName(''); setUrl(''); setPassword(''); setError(''); setBusy(false);
  }, [open]);

  const addDesktop = async () => {
    setError(''); setBusy(true);
    try {
      const device = await invoke('lan_add_manual', { address });
      onLanAdded(device);
      onClose();
    } catch (e) {
      setError(String(e?.message || e));
    } finally { setBusy(false); }
  };

  const addBackend = async () => {
    setError(''); setBusy(true);
    try {
      const base = await invoke('remote_backend_test', { url: url.trim(), password });
      await invoke('cloud_sync_remember', { password, url: base });
      onBackendAdded({ id: `rb-${Date.now()}`, name: name.trim() || hostOf(base), url: base });
      onClose();
    } catch (e) {
      setError(String(e?.message || e));
    } finally { setBusy(false); }
  };

  const isBackend = mode === 'backend';
  const canSubmit = isBackend ? !!url.trim() : !!address.trim() && lanEnabled;

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{isBackend ? 'Add remote backend' : 'Add machine to sync with'}</DialogTitle>
          <DialogDescription>
            {isBackend
              ? 'A running rustic-server you can open in its own window. It also appears in Sync.'
              : 'Desktops on your network appear automatically — add one here if it doesn\u2019t, or connect over the internet.'}
          </DialogDescription>
        </DialogHeader>
        {!isBackend && (
          <Slider
            value={tab}
            onChange={(v) => { setTab(v); setError(''); }}
            options={[
              { id: 'lan', label: 'Local network', icon: Wifi },
              { id: 'internet', label: 'Over the internet', icon: Globe },
            ]}
          />
        )}
        {isBackend ? (
          <div className="space-y-2">
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Name (optional)" className="h-8 text-xs" />
            <Input autoFocus value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://rustic.example.com" className="h-8 font-mono text-xs" />
            <Input
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && canSubmit && addBackend()}
              placeholder="Server password"
              className="h-8 text-xs"
            />
            <p className="text-[10.5px] text-muted-foreground">Verified, then saved in your OS keychain for this backend only.</p>
          </div>
        ) : (
          <div className="space-y-2">
            {!lanEnabled && <p className="text-[11.5px] text-amber-500">Turn on local-network sync under My machine first.</p>}
            <Input
              autoFocus
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && canSubmit && addDesktop()}
              placeholder={tab === 'lan' ? '192.168.1.20  or  192.168.1.20:47820' : 'https://xyz.trycloudflare.com  or  203.0.113.7:47820'}
              className="h-8 font-mono text-xs"
            />
            <p className="text-[10.5px] text-muted-foreground">
              {tab === 'lan'
                ? 'The other machine shows its address under My machine.'
                : 'On the other machine, turn on the Cloudflare tunnel under My machine and paste its URL here — or forward port 47820 on its router and enter its public IP.'}
              {' '}You'll pair next, confirming the same 6-digit code on both screens.
            </p>
          </div>
        )}
        {error && <p className="break-all text-[11px] text-rose-500">{error}</p>}
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="h-7 text-xs" disabled={busy || !canSubmit} onClick={isBackend ? addBackend : addDesktop}>
            {busy ? <Loader2 className="size-3 animate-spin" /> : <Plus className="size-3" />}
            {isBackend ? 'Verify & save' : 'Find machine'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}