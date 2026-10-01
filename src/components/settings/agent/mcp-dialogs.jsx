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
  stdio: { mcpServers: { 'server-name': { command: 'npx', args: ['-y', '<package>'], env: { API_KEY: '' } } } },
  http: { mcpServers: { 'server-name': { url: 'https://example.com/mcp', headers: { Authorization: 'Bearer <token>' } } } },
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

const isObj = (v) => !!v && typeof v === 'object' && !Array.isArray(v);
const isEntry = (v) => isObj(v) && (typeof v.command === 'string' || typeof v.url === 'string');

/** Parse standard MCP JSON ({"mcpServers":{...}}, {"name":{...}} or a bare entry) into [name, entry] pairs. */
function parseServers(text, name) {
  let v;
  try { v = JSON.parse(text); } catch (e) { return { error: `Invalid JSON: ${e.message}` }; }
  if (!isObj(v)) return { error: 'Config must be a JSON object' };
  let pairs;
  if ('mcpServers' in v) {
    if (!isObj(v.mcpServers)) return { error: '"mcpServers" must be an object' };
    pairs = Object.entries(v.mcpServers);
  } else if (isEntry(v)) {
    if (!name) return { error: 'Give the server a name, or wrap it as {"mcpServers": {"<name>": {...}}}' };
    pairs = [[name, v]];
  } else if (Object.keys(v).length && Object.values(v).every(isEntry)) {
    pairs = Object.entries(v);
  } else {
    return { error: 'Expected {"mcpServers": {"<name>": {"command": ..., "args": [...]}}} or an entry with "command"/"url"' };
  }
  if (!pairs.length) return { error: '"mcpServers" has no servers' };
  const bad = pairs.find(([, e]) => !isEntry(e));
  if (bad) return { error: `Server "${bad[0]}" needs a "command" (stdio) or a "url" (http/sse)` };
  return { pairs };
}

/** Parse editor text into a single server entry for `name`, or return an error string. */
function parseEntry(text, name) {
  const { pairs, error } = parseServers(text, name);
  if (error) return { error };
  const hit = pairs.length === 1 ? pairs[0] : pairs.find(([k]) => k === name);
  if (!hit) return { error: `Config must contain exactly one server (or one named "${name}")` };
  return { entry: hit[1] };
}

/** Sliding two-option toggle (Tools / Projects) heading the right-hand pane. */
function PaneSwitch({ value, onChange, options }) {
  const idx = Math.max(0, options.findIndex((o) => o.id === value));
  return (
    <div className="relative grid grid-cols-2 rounded-md border border-border/60 bg-muted/30 p-0.5">
      <span
        aria-hidden
        className="absolute inset-y-0.5 left-0.5 w-[calc(50%-2px)] rounded bg-background shadow-sm transition-transform duration-200 ease-out"
        style={{ transform: `translateX(${idx * 100}%)` }}
      />
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          onClick={() => onChange(o.id)}
          className={cn(
            'relative z-10 flex items-center justify-center gap-1.5 rounded px-3 py-1.5 text-[12px] transition-colors',
            value === o.id ? 'text-foreground' : 'text-muted-foreground hover:text-foreground',
          )}
        >
          {o.label}
          {o.count != null && (
            <span className="rounded-full bg-muted px-1.5 text-[10px] tabular-nums text-muted-foreground">{o.count}</span>
          )}
        </button>
      ))}
    </div>
  );
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
  const [testing, setTesting] = useState(false);
  const [tested, setTested] = useState(null); // [{ name, tools?, error? }]
  const [pane, setPane] = useState('projects');

  useEffect(() => {
    if (!open) return;
    setName(''); setKind('stdio'); setJson(pretty(TEMPLATES.stdio)); setError(''); setTested(null); setPane('projects');
    setSelected(Object.fromEntries((projects || []).map((p) => [p.id, true])));
  }, [open, projects]);

  const pickTemplate = (k) => { setKind(k); setJson(pretty(TEMPLATES[k])); setTested(null); };
  const allOn = (projects || []).every((p) => selected[p.id]);

  const runTest = async () => {
    setError('');
    const { pairs, error: parseErr } = parseServers(json, name.trim());
    if (parseErr) { setError(parseErr); return; }
    if (!isTauri()) return;
    setTesting(true);
    setTested(null);
    const out = [];
    for (const [serverName, entry] of pairs) {
      try {
        const tools = await invoke('test_mcp_entry', { name: serverName, entry });
        out.push({ name: serverName, tools: Array.isArray(tools) ? tools : [] });
      } catch (e) {
        out.push({ name: serverName, error: String(e) });
      }
    }
    setTested(out);
    setTesting(false);
    setPane('tools');
  };

  const submit = async () => {
    setError('');
    const { pairs, error: parseErr } = parseServers(json, name.trim());
    if (parseErr) { setError(parseErr); return; }
    if (pairs.some(([n]) => n === 'server-name')) { setError('Replace "server-name" with a real server name'); return; }
    if (!isTauri()) return;
    setSaving(true);
    try {
      const projectIds = Object.entries(selected).filter(([, v]) => v).map(([k]) => k);
      for (const [serverName, entry] of pairs) {
        const res = await invoke('add_mcp_pool_server', { name: serverName, entry, projectIds });
        const failedProjects = (res?.projects || []).filter((p) => p.error);
        if (res?.pool?.connected) {
          toast.success(`Added ${serverName} · ${res.pool.toolCount} tool${res.pool.toolCount === 1 ? '' : 's'}`);
        } else {
          toast.error(`Added ${serverName}, but it failed to connect: ${res?.pool?.error || 'unknown error'}`);
        }
        if (failedProjects.length) {
          toast.error(`Couldn't write ${serverName} to ${failedProjects.map((p) => p.projectName).join(', ')}: ${failedProjects[0].error}`);
        }
      }
      onClose(true);
    } catch (e) {
      setError(String(e));
    } finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose(false)}>
      <DialogContent aria-describedby={undefined} className="w-[min(1180px,94vw)] sm:max-w-[min(1180px,94vw)] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Add MCP server</DialogTitle>
        </DialogHeader>
        <div className="flex h-[min(620px,72vh)] min-w-0">
          <div className="flex min-w-0 flex-1 flex-col gap-3 border-r border-border/60 p-4">
            <div className="flex items-center gap-2">
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="name (optional — taken from mcpServers keys)"
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
            <JsonArea value={json} onChange={(v) => { setJson(v); setTested(null); }} className="h-auto min-h-0 flex-1" />
            <div className="flex items-center gap-2">
              <Button size="sm" variant="outline" className="h-7 text-xs" onClick={runTest} disabled={testing}>
                {testing ? <><Loader2 className="mr-1 size-3 animate-spin" /> Testing…</> : 'Test'}
              </Button>
              <span className="text-[10.5px] text-muted-foreground">Paste a standard <code>{'{"mcpServers": {...}}'}</code> block — several servers are added at once.</span>
            </div>
            {error && <p className="break-all text-[11px] text-rose-500">{error}</p>}
          </div>
          <div className="flex w-[440px] shrink-0 flex-col gap-3 overflow-hidden p-4">
            <PaneSwitch
              value={pane}
              onChange={setPane}
              options={[
                { id: 'tools', label: 'Tools', count: tested ? tested.reduce((n, t) => n + (t.tools?.length || 0), 0) : null },
                { id: 'projects', label: 'Projects', count: (projects || []).length ? Object.values(selected).filter(Boolean).length : null },
              ]}
            />
            {pane === 'tools' ? (
              <div className="min-h-0 flex-1 space-y-2 overflow-y-auto rounded-md border border-border/50 p-3">
                {!tested && !testing && (
                  <div className="text-[11.5px] text-muted-foreground">Run Test to connect and list the server's tools.</div>
                )}
                {testing && (
                  <div className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground"><Loader2 className="size-3 animate-spin" /> Connecting…</div>
                )}
                {(tested || []).map((t) => (
                  <div key={t.name} className="space-y-1.5">
                    {tested.length > 1 && <div className="font-mono text-[11.5px] font-medium">{t.name}</div>}
                    {t.error ? (
                      <div className="break-all text-[11.5px] text-rose-500">Failed: {t.error}</div>
                    ) : t.tools.length === 0 ? (
                      <div className="text-[11.5px] text-muted-foreground">Connected · no tools advertised.</div>
                    ) : (
                      <>
                        <div className="text-[11px] text-emerald-500">Connected · {t.tools.length} tool{t.tools.length === 1 ? '' : 's'}</div>
                        {t.tools.map((tool) => (
                          <div key={tool.name} className="rounded-md px-1 py-0.5 hover:bg-muted/40" title={tool.description || undefined}>
                            <div className="truncate font-mono text-[12px] text-foreground/90">{tool.name}</div>
                            {tool.description && (
                              <div className="line-clamp-2 text-[11px] leading-snug text-muted-foreground">{tool.description}</div>
                            )}
                          </div>
                        ))}
                      </>
                    )}
                  </div>
                ))}
              </div>
            ) : (
              <div className="flex min-h-0 flex-1 flex-col">
                {(projects || []).length > 0 && (
                  <div className="mb-1.5 flex justify-end">
                    <button
                      type="button"
                      className="text-[11px] text-muted-foreground hover:text-foreground"
                      onClick={() => setSelected(Object.fromEntries((projects || []).map((p) => [p.id, !allOn])))}
                    >
                      {allOn ? 'Clear all' : 'Select all'}
                    </button>
                  </div>
                )}
                <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto rounded-md border border-border/50 p-1">
                  {(projects || []).length === 0 && (
                    <li className="px-2 py-1.5 text-[11.5px] text-muted-foreground">No projects open — the server is added to the global pool only.</li>
                  )}
                  {(projects || []).map((p) => (
                    <li key={p.id} className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted/50">
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-[12px]">{p.name}</div>
                        <div className="truncate font-mono text-[10.5px] text-muted-foreground">{p.root_path}</div>
                      </div>
                      <Switch
                        checked={!!selected[p.id]}
                        onCheckedChange={(v) => setSelected((s) => ({ ...s, [p.id]: v }))}
                        aria-label={`Enable in ${p.name}`}
                      />
                    </li>
                  ))}
                </ul>
                <p className="mt-1.5 text-[10.5px] text-muted-foreground">
                  Written to each enabled project's <code>.mcp.json</code>, <code>.gemini/settings.json</code> and <code>.codex/config.toml</code> (git-ignored).
                </p>
              </div>
            )}
          </div>
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
  const [tools, setTools] = useState(null);
  const [toolsError, setToolsError] = useState('');
  const [toggling, setToggling] = useState(null);
  const [pane, setPane] = useState('projects');

  const selected = useMemo(() => rows.find((r) => r.projectId === selectedId), [rows, selectedId]);
  const selectedKey = selected ? `${selected.projectId}:${JSON.stringify(selected.view?.entry ?? null)}:${selected.view?.enabled}` : null;

  const loadTools = useCallback(async () => {
    const id = server?.id || server?.name;
    if (!open || !id || !isTauri()) return;
    setToolsError('');
    try {
      const list = await invoke('list_mcp_server_tools', { id });
      setTools(Array.isArray(list) ? list : []);
    } catch (e) {
      setTools([]);
      setToolsError(String(e));
    }
  }, [open, server?.id, server?.name]);

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
    setResult(null); setError(''); setPending(null); setTools(null);
    load(null);
    loadTools();
  }, [open, load, loadTools]);

  useEffect(() => {
    if (!selected) return;
    setJson(pretty(selected.view?.entry ?? {}));
    setEnabled(selected.view?.enabled ?? true);
    setResult(null); setError(''); setPending(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedKey]);

  const toggleProject = async (row, on) => {
    if (!isTauri()) return;
    setToggling(row.projectId); setError('');
    try {
      await invoke('save_mcp_project_server', {
        name, projectId: row.projectId, entry: on ? (row.view?.entry ?? null) : null, enabled: on,
      });
      await load(selectedId);
    } catch (e) {
      setError(String(e));
    } finally { setToggling(null); }
  };

  const save = async () => {
    if (!selected || !isTauri()) return;
    setError(''); setResult(null); setPending(null);
    let entry = null;
    if (enabled) {
      const parsed = parseEntry(json, name);
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
      if (res?.connected) loadTools();
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
      <DialogContent aria-describedby={undefined} className="w-[min(1180px,94vw)] sm:max-w-[min(1180px,94vw)] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">
            Configure <span className="font-mono">{name}</span> per project
          </DialogTitle>
        </DialogHeader>
        <div className="flex h-[min(620px,72vh)] min-w-0">
          <div className="flex min-w-0 flex-1 flex-col gap-3 overflow-y-auto border-r border-border/60 p-4">
            {selected ? (
              <>
                <div className="min-w-0">
                  <div className="text-[10.5px] uppercase tracking-wide text-muted-foreground">Config for</div>
                  <div className="truncate text-[12px] font-medium">{selected.projectName}</div>
                  <div className="truncate font-mono text-[10.5px] text-muted-foreground">{selected.rootPath}</div>
                </div>
                {selected.error && <p className="break-all text-[11px] text-rose-500">{selected.error}</p>}
                <JsonArea value={json} onChange={setJson} disabled={!enabled} className="h-auto min-h-[200px] flex-1" />
                <div className="flex items-center gap-2">
                  <Button size="sm" className="h-7 text-xs" onClick={save} disabled={!enabled || saving}>
                    {saving ? <><Loader2 className="mr-1 size-3 animate-spin" /> Testing…</> : 'Save & test'}
                  </Button>
                  {!enabled && <span className="text-[11px] text-muted-foreground">Disabled for this project — enable it on the right to edit.</span>}
                </div>
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
              <div className="text-[12px] text-muted-foreground">
                {loading ? 'Loading…' : rows.length === 0 ? 'No projects open in Rustic.' : 'Select a project on the right.'}
              </div>
            )}
          </div>
          <div className="flex w-[440px] shrink-0 flex-col gap-3 overflow-hidden p-4">
            <PaneSwitch
              value={pane}
              onChange={setPane}
              options={[
                { id: 'tools', label: 'Tools', count: tools ? tools.length : null },
                { id: 'projects', label: 'Active projects', count: rows.length ? rows.filter((r) => r.view?.enabled ?? true).length : null },
              ]}
            />
            {pane === 'tools' ? (
              <ul className="min-h-0 flex-1 space-y-1 overflow-y-auto rounded-md border border-border/50 p-3">
                {tools === null && <li className="text-[11.5px] text-muted-foreground">Loading…</li>}
                {tools && tools.length === 0 && (
                  <li className="break-all text-[11.5px] text-muted-foreground">
                    {toolsError ? 'Not connected — run Save & test to load tools.' : 'No tools advertised.'}
                  </li>
                )}
                {(tools || []).map((t) => (
                  <li key={t.name} className="rounded-md px-1 py-0.5 hover:bg-muted/40" title={t.description || undefined}>
                    <div className="truncate font-mono text-[12px] text-foreground/90">{t.name}</div>
                    {t.description && (
                      <div className="line-clamp-2 text-[11px] leading-snug text-muted-foreground">{t.description}</div>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <div className="flex min-h-0 flex-1 flex-col">
              <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto rounded-md border border-border/50 p-1">
                {!loading && rows.length === 0 && (
                  <li className="px-2 py-1.5 text-[11px] text-muted-foreground">No projects open in Rustic.</li>
                )}
                {rows.map((r) => {
                  const on = r.view?.enabled ?? true;
                  const b = statusBadge(r.view?.status, on);
                  return (
                    <li
                      key={r.projectId}
                      className={cn(
                        'flex items-center gap-2 rounded-md px-2 py-1.5',
                        r.projectId === selectedId ? 'bg-muted' : 'hover:bg-muted/50',
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => setSelectedId(r.projectId)}
                        className="min-w-0 flex-1 text-left"
                        title="Edit this project's config"
                      >
                        <div className="truncate text-[12px]">{r.projectName}</div>
                        <div className="mt-0.5 flex items-center gap-1">
                          <Badge variant="outline" className={cn('h-4 text-[9.5px]', b.tone)}>{b.label}</Badge>
                          {r.view?.overridden && (
                            <Badge variant="outline" className="h-4 text-[9.5px] text-muted-foreground">custom</Badge>
                          )}
                        </div>
                      </button>
                      {toggling === r.projectId ? (
                        <Loader2 className="size-3.5 animate-spin text-muted-foreground" />
                      ) : (
                        <Switch
                          checked={on}
                          disabled={!!toggling || !r.view}
                          onCheckedChange={(v) => toggleProject(r, v)}
                          aria-label={`Enable ${name} in ${r.projectName}`}
                        />
                      )}
                    </li>
                  );
                })}
              </ul>
              </div>
            )}
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
