// MCP servers section.
// Split out of agent-settings.jsx (A4).
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronRight, ChevronDown, Plus, Eye, EyeOff, Pencil, Trash2, Info, RefreshCw,
  ClipboardEdit, X, Check, FileText, Copy, List, Loader2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem, SelectGroup, SelectLabel,
} from '@/components/ui/select';
import { ScrollArea } from '@/components/ui/scroll-area';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, isTauri } from './shared';

// ─── MCP Servers ─────────────────────────────────────────────────────────────

// MCP servers are global by design — the user-scope `.mcp.json` is the single
// source of truth, applied across every project. The dialog used to support a
// project-scoped variant but that was removed to keep the UX simple: one set
// of servers, configured once.
export function McpJsonDialog({ open, onClose }) {
  const [json, setJson] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open || !isTauri()) return;
    invoke('read_mcp_json', { scope: 'user', projectId: null })
      .then((t) => setJson(typeof t === 'string' ? t : ''))
      .catch(() => setJson('{\n  "mcpServers": {}\n}'));
  }, [open]);

  const save = async () => {
    try { JSON.parse(json); }
    catch { toast.error('Invalid JSON'); return; }
    setSaving(true);
    try {
      const results = await invoke('save_mcp_json', { scope: 'user', projectId: null, content: json });
      const failed = (results || []).filter((r) => !r.connected);
      if (failed.length === 0) toast.success('MCP saved');
      else toast.error(`Saved, but ${failed.length} server(s) failed to connect`);
      onClose();
    } catch (e) { toast.error(String(e)); }
    finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Edit mcp.json</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4">
          <Textarea
            value={json}
            onChange={(e) => setJson(e.target.value)}
            className="min-h-[320px] font-mono text-[11px] resize-none"
            spellCheck={false}
          />
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={saving}>
            {saving ? 'Saving…' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function McpServerRow({ server, onRemove }) {
  const [open, setOpen] = useState(false);
  const [tools, setTools] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const id = server.id || server.name;
  const st = server.status || { state: 'unknown' };
  const connected = st.state === 'connected';
  const label =
    connected ? `Connected · ${st.tool_count ?? 0} tool${st.tool_count === 1 ? '' : 's'}` :
    st.state === 'failed' ? 'Failed' :
    'Idle';
  const tone =
    connected ? 'border-emerald-500/40 text-emerald-500' :
    st.state === 'failed' ? 'border-rose-500/40 text-rose-500' :
    'border-border/60 text-muted-foreground';

  const toggle = async () => {
    const next = !open;
    setOpen(next);
    if (next && tools === null && connected) {
      setLoading(true); setError('');
      try {
        const list = await invoke('list_mcp_server_tools', { id });
        setTools(Array.isArray(list) ? list : []);
      } catch (e) {
        setError(String(e));
        setTools([]);
      } finally { setLoading(false); }
    }
  };

  return (
    <li className="rounded-md border border-border/50 bg-muted/30 overflow-hidden">
      <div
        className={cn(
          'flex items-center gap-2 px-3 py-2',
          connected && 'cursor-pointer hover:bg-muted/50'
        )}
        onClick={() => connected && toggle()}
      >
        {connected ? (
          <ChevronRight className={cn('size-3.5 text-muted-foreground transition-transform shrink-0', open && 'rotate-90')} />
        ) : (
          <span className="w-3.5 shrink-0" />
        )}
        <span className="text-[12px] font-mono flex-1 truncate">{server.name || id}</span>
        <Badge variant="outline" className={cn('h-5 text-[10px]', tone)}>{label}</Badge>
        <Button
          size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-destructive"
          onClick={(e) => { e.stopPropagation(); onRemove(id); }}
        >
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      {st.state === 'failed' && st.error && (
        <p className="px-3 pb-2 text-[11px] text-rose-500/90 break-all">{st.error}</p>
      )}
      {open && connected && (
        <div className="border-t border-border/40 bg-muted/10">
          {loading ? (
            <div className="px-3 py-2 text-[11px] text-muted-foreground">Loading tools…</div>
          ) : error ? (
            <div className="px-3 py-2 text-[11px] text-destructive break-all">{error}</div>
          ) : (tools || []).length === 0 ? (
            <div className="px-3 py-2 text-[11px] text-muted-foreground">No tools advertised.</div>
          ) : (
            <ul className="divide-y divide-border/30">
              {tools.map((t) => (
                <li key={t.name} className="px-3 py-2">
                  <div className="text-[12px] font-mono text-foreground/90">{t.name}</div>
                  {t.description && (
                    <div className="mt-0.5 text-[11px] italic text-muted-foreground leading-snug whitespace-pre-wrap">
                      {t.description}
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </li>
  );
}

export function McpSection() {
  const [servers, setServers] = useState([]);
  const [jsonOpen, setJsonOpen] = useState(false);

  // MCP servers are configured once at the user level and apply across all
  // projects — no per-project scoping. We pass projectId: null so the backend
  // always returns / writes the user-level server list.
  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_mcp_servers', { projectId: null });
      setServers(Array.isArray(list) ? list : []);
    } catch { setServers([]); }
  };
  useEffect(() => { refresh(); }, []);

  const remove = async (id) => {
    try { await invoke('remove_mcp_server', { id }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="MCP Servers"
      badge="Global"
      actions={
        <>
          <Button size="sm" variant="outline" className="h-7 text-xs gap-1.5" onClick={() => setJsonOpen(true)}>
            <ClipboardEdit className="size-3" /> Edit JSON
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setJsonOpen(true)}>
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {servers.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">
          No MCP servers configured.<br />
          Click "Edit JSON" to add one. Standard <code className="text-[11px]">.mcp.json</code> format.
        </div>
      ) : (
        <ul className="space-y-1.5">
          {servers.map((s) => (
            <McpServerRow key={s.id || s.name} server={s} onRemove={remove} />
          ))}
        </ul>
      )}
      <McpJsonDialog open={jsonOpen} onClose={() => { setJsonOpen(false); refresh(); }} />
    </Section>
  );
}

