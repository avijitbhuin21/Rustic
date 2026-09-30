// MCP dialogs: add one server to the global pool, and configure a pool
// server per project (entry JSON, enable/disable, connection test).
import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Loader2, ShieldAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useExplorer } from '@/state/explorer';
import { isTauri } from './shared';

const TEMPLATES = {
  stdio: { command: 'npx', args: ['-y', '<package>'], env: { API_KEY: '' } },
  http: { type: 'http', url: 'https://example.com/mcp', headers: { Authorization: 'Bearer <token>' } },
};

const pretty = (v) => JSON.stringify(v, null, 2);

/** Fixed-size, horizontally scrolling JSON editor that never grows its dialog. */
export function JsonArea({ value, onChange, disabled, className }) {
  return (
    <Textarea
      value={value}
      onChange={(e) => onChange?.(e.target.value)}
      disabled={disabled}
      readOnly={!onChange}
      spellCheck={false}
      wrap="off"
      className={cn(
        'field-sizing-fixed h-[320px] min-h-0 w-full min-w-0 resize-none overflow-auto whitespace-pre font-mono text-[11px] leading-relaxed',
        className,
      )}
    />
  );
}

/** Parse editor text into a server entry object, or return an error string. */
function parseEntry(text) {
  let v;
  try { v = JSON.parse(text); } catch (e) { return { error: `Invalid JSON: ${e.message}` }; }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return { error: 'Entry must be a JSON object' };
  if (typeof v.command !== 'string' && typeof v.url !== 'string') {
    return { error: 'Entry needs a "command" (stdio) or a "url" (http/sse)' };
  }
  return { entry: v };
}

/** Short label + tone for a connection status object from the backend. */
export function statusBadge(status, enabled = true) {
  if (!enabled) return { label: 'Disabled', tone: 'border-border/60 text-muted-foreground' };
  const st = status || { state: 'unknown' };
  if (st.state === 'connected') {
    const n = st.tool_count ?? 0;
    return { label: `Connected · ${n} tool${n === 1 ? '' : 's'}`, tone: 'border-emerald-500/40 text-emerald-500' };
  }
  if (st.state === 'failed') return { label: 'Failed', tone: 'border-rose-500/40 text-rose-500' };
  return { label: 'Idle', tone: 'border-border/60 text-muted-foreground' };
}

/** "+" flow: add a single new server to the global pool and the ticked projects. */
export function McpAddServerDialog({ open, onClose }) {
  const projects = useExplorer((s) => s.projects);
  const [name, setName] = useState('');
  const [kind, setKind] = useState('stdio');
  const [json, setJson] = useState(pretty(TEMPLATES.stdio));
  const [selected, setSelected] = useState({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!open) return;
    setName(''); setKind('stdio'); setJson(pretty(TEMPLATES.stdio)); setError('');
    setSelected(Object.fromEntries((projects || []).map((p) => [p.id, true])));
  }, [open, projects]);

  const pickTemplate = (k) => { setKind(k); setJson(pretty(TEMPLATES[k])); };
  const allOn = (projects || []).every((p) => selected[p.id]);

  const submit = async () => {
    setError('');
    const trimmed = name.trim();
    if (!trimmed) { setError('Give the server a name'); return; }
    const { entry, error: parseErr } = parseEntry(json);
    if (parseErr) { setError(parseErr); return; }
    if (!isTauri()) return;
    setSaving(true);
    try {
      const projectIds = Object.entries(selected).filter(([, v]) => v).map(([k]) => k);
      const res = await invoke('add_mcp_pool_server', { name: trimmed, entry, projectIds });
      const failedProjects = (res?.projects || []).filter((p) => p.error);
      if (res?.pool?.connected) {
        toast.success(`Added ${trimmed} · ${res.pool.toolCount} tool${res.pool.toolCount === 1 ? '' : 's'}`);
      } else {
        toast.error(`Added ${trimmed}, but it failed to connect: ${res?.pool?.error || 'unknown error'}`);
      }
      if (failedProjects.length) {
        toast.error(`Couldn't write to ${failedProjects.map((p) => p.projectName).join(', ')}: ${failedProjects[0].error}`);
      }
      onClose(true);
    } catch (e) {
      setError(String(e));
    } finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose(false)}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Add MCP server</DialogTitle>
        </DialogHeader>
        <div className="min-w-0 space-y-3 px-5 py-4">
          <div className="flex items-center gap-2">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="server-name"
              className="h-8 flex-1 font-mono text-xs"
              autoFocus
            />
            <div className="flex rounded-md border border-border/60 p-0.5">
              {['stdio', 'http'].map((k) => (
                <button
                  key={k}
                  type="button"
                  onClick={() => pickTemplate(k)}
                  className={cn(
                    'rounded px-2.5 py-1 text-[11px] font-mono',
                    kind === k ? 'bg-muted text-foreground' : 'text-muted-foreground hover:text-foreground',
                  )}
                >
                  {k}
                </button>
              ))}
            </div>
          </div>
          <JsonArea value={json} onChange={setJson} className="h-[220px]" />
          <div>
            <div className="mb-1.5 flex items-center justify-between">
              <span className="text-[11px] font-medium text-muted-foreground">Enable in projects</span>
              <button
                type="button"
                className="text-[11px] text-muted-foreground hover:text-foreground"
                onClick={() => setSelected(Object.fromEntries((projects || []).map((p) => [p.id, !allOn])))}
              >
                {allOn ? 'Clear all' : 'Select all'}
              </button>
            </div>
            <div className="max-h-[140px] space-y-1 overflow-y-auto rounded-md border border-border/50 p-2">
              {(projects || []).length === 0 && (
                <div className="text-[11px] text-muted-foreground">No projects open — the server is added to the global pool only.</div>
              )}
              {(projects || []).map((p) => (
                <label key={p.id} className="flex cursor-pointer items-center gap-2 text-[12px]">
                  <input
                    type="checkbox"
                    className="accent-foreground"
                    checked={!!selected[p.id]}
                    onChange={(e) => setSelected((s) => ({ ...s, [p.id]: e.target.checked }))}
                  />
                  <span className="truncate">{p.name}</span>
                  <span className="ml-auto truncate font-mono text-[10px] text-muted-foreground">{p.root_path}</span>
                </label>
              ))}
            </div>
            <p className="mt-1.5 text-[10.5px] text-muted-foreground">
              Written to each project's <code>.mcp.json</code>, <code>.gemini/settings.json</code> and <code>.codex/config.toml</code> (git-ignored).
            </p>
          </div>
          {error && <p className="break-all text-[11px] text-rose-500">{error}</p>}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={() => onClose(false)}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={submit} disabled={saving}>
            {saving ? <><Loader2 className="mr-1 size-3 animate-spin" /> Adding…</> : 'Add server'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Per-project configuration for one pool server. */
export function McpConfigureDialog({ server, open, onClose }) {
  const name = server?.name;
  const [rows, setRows] = useState([]);
  const [loading, setLoading] = useState(false);
  const [selectedId, setSelectedId] = useState(null);
  const [json, setJson] = useState('');
  const [enabled, setEnabled] = useState(true);
  const [saving, setSaving] = useState(false);
  const [result, setResult] = useState(null);
  const [error, setError] = useState('');
  const [pending, setPending] = useState(null);

  const selected = useMemo(() => rows.find((r) => r.projectId === selectedId), [rows, selectedId]);

  const load = useCallback(async (keepId) => {
    if (!open || !name || !isTauri()) return;
    setLoading(true);
    try {
      const list = await invoke('get_mcp_server_projects', { name });
      const next = Array.isArray(list) ? list : [];
      setRows(next);
      const id = keepId && next.some((r) => r.projectId === keepId) ? keepId : next[0]?.projectId ?? null;
      setSelectedId(id);
    } catch (e) {
      setError(String(e));
    } finally { setLoading(false); }
  }, [open, name]);

  useEffect(() => {
    if (!open) return;
    setResult(null); setError(''); setPending(null);
    load(null);
  }, [open, load]);

  useEffect(() => {
    if (!selected) return;
    setJson(pretty(selected.view?.entry ?? {}));
    setEnabled(selected.view?.enabled ?? true);
    setResult(null); setError(''); setPending(null);
  }, [selected]);

  const save = async () => {
    if (!selected || !isTauri()) return;
    setError(''); setResult(null); setPending(null);
    let entry = null;
    if (enabled) {
      const parsed = parseEntry(json);
      if (parsed.error) { setError(parsed.error); return; }
      entry = parsed.entry;
    }
    setSaving(true);
    try {
      const res = await invoke('save_mcp_project_server', {
        name, projectId: selected.projectId, entry, enabled,
      });
      setResult(res);
      await load(selected.projectId);
    } catch (e) {
      setError(String(e));
    } finally { setSaving(false); }
  };

  const reviewConsent = async () => {
    if (!selected) return;
    try {
      setPending(await invoke('get_pending_mcp_consent', { projectId: selected.projectId }));
    } catch (e) { setError(String(e)); }
  };

  const approveConsent = async () => {
    if (!selected || !pending) return;
    try {
      await invoke('approve_mcp_project_consent', {
        projectId: selected.projectId,
        contentHash: pending.contentHash || pending.content_hash,
      });
      setPending(null);
      await save();
    } catch (e) { setError(String(e)); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[860px] sm:max-w-[860px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">
            Configure <span className="font-mono">{name}</span> per project
          </DialogTitle>
        </DialogHeader>
        <div className="flex min-h-[420px] min-w-0">
          <ul className="w-60 shrink-0 space-y-0.5 overflow-y-auto border-r border-border/60 p-2">
            {loading && rows.length === 0 && (
              <li className="px-2 py-1.5 text-[11px] text-muted-foreground">Loading…</li>
            )}
            {!loading && rows.length === 0 && (
              <li className="px-2 py-1.5 text-[11px] text-muted-foreground">No projects open in Rustic.</li>
            )}
            {rows.map((r) => {
              const b = statusBadge(r.view?.status, r.view?.enabled ?? true);
              return (
                <li key={r.projectId}>
                  <button
                    type="button"
                    onClick={() => setSelectedId(r.projectId)}
                    className={cn(
                      'w-full rounded-md px-2 py-1.5 text-left',
                      r.projectId === selectedId ? 'bg-muted' : 'hover:bg-muted/50',
                    )}
                  >
                    <div className="truncate text-[12px]">{r.projectName}</div>
                    <div className="mt-0.5 flex items-center gap-1">
                      <Badge variant="outline" className={cn('h-4 text-[9.5px]', b.tone)}>{b.label}</Badge>
                      {r.view?.overridden && (
                        <Badge variant="outline" className="h-4 text-[9.5px] text-muted-foreground">custom</Badge>
                      )}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
          <div className="min-w-0 flex-1 space-y-3 p-4">
            {selected ? (
              <>
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-[12px] font-medium">{selected.projectName}</div>
                    <div className="truncate font-mono text-[10.5px] text-muted-foreground">{selected.rootPath}</div>
                  </div>
                  <label className="flex shrink-0 items-center gap-2 text-[11.5px]">
                    Enabled
                    <Switch checked={enabled} onCheckedChange={setEnabled} />
                  </label>
                </div>
                {selected.error && <p className="break-all text-[11px] text-rose-500">{selected.error}</p>}
                <JsonArea value={json} onChange={setJson} disabled={!enabled} className="h-[270px]" />
                <p className="text-[10.5px] text-muted-foreground">
                  Saved to this project's <code>.mcp.json</code>, <code>.gemini/settings.json</code> and{' '}
                  <code>.codex/config.toml</code>. Disabling removes it from those files for this project only.
                </p>
                {result && !result.consentRequired && (
                  <div
                    className={cn(
                      'rounded-md border px-3 py-2 text-[11.5px]',
                      !result.enabled ? 'border-border/60 text-muted-foreground'
                        : result.connected ? 'border-emerald-500/40 text-emerald-500'
                          : 'border-rose-500/40 text-rose-500',
                    )}
                  >
                    {!result.enabled
                      ? 'Disabled for this project.'
                      : result.connected
                        ? `Connected · ${result.toolCount} tool${result.toolCount === 1 ? '' : 's'} available.`
                        : <span className="break-all">Connection failed: {result.error || 'unknown error'}</span>}
                  </div>
                )}
                {result?.consentRequired && (
                  <div className="space-y-2 rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-[11.5px]">
                    <div className="flex items-center gap-2 text-amber-500">
                      <ShieldAlert className="size-3.5 shrink-0" />
                      This project's .mcp.json has changes made outside Rustic that you haven't approved yet.
                    </div>
                    {pending ? (
                      <>
                        <JsonArea value={pending.content || ''} className="h-[140px]" />
                        <Button size="sm" className="h-7 text-xs" onClick={approveConsent}>Approve and test</Button>
                      </>
                    ) : (
                      <Button size="sm" variant="outline" className="h-7 text-xs" onClick={reviewConsent}>Review file</Button>
                    )}
                  </div>
                )}
                {error && <p className="break-all text-[11px] text-rose-500">{error}</p>}
              </>
            ) : (
              <div className="text-[12px] text-muted-foreground">Select a project.</div>
            )}
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Close</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={!selected || saving}>
            {saving ? <><Loader2 className="mr-1 size-3 animate-spin" /> Testing…</> : 'Save & test'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
