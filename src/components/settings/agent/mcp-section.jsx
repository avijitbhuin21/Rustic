// MCP servers section.
// Split out of agent-settings.jsx (A4).
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronRight, ChevronDown, Plus, Eye, EyeOff, Pencil, Trash2, Info, RefreshCw,
  ClipboardEdit, X, Check, FileText, Copy, List, Loader2, Settings2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { InfoTip } from '@/components/ui/info-tip';
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
import { Skeleton } from '@/components/ui/skeleton';
import { toast } from 'sonner';
import { toastError } from '@/lib/toast-error';
import { cn } from '@/lib/utils';
import { applyMcpStatus, mcpStatusReady, reconcileMcpList, useMcpServerStatus } from '@/lib/mcp-status';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, isTauri, useExtensionsChanged } from './shared';
import { RowGroup } from '../setting-row';
import { JsonArea, McpAddServerDialog, McpConfigureDialog, statusBadge } from './mcp-dialogs';

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
      await invoke('save_mcp_json', { scope: 'user', projectId: null, content: json });
      toast.success('MCP saved');
      onClose();
    } catch (e) { toastError(null, e); }
    finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="flex items-center gap-1.5 text-[14px]">
            Edit mcp.json
            <InfoTip side="bottom">Global pool — every server here is available to all projects. Use Configure on a server to change it per project.</InfoTip>
          </DialogTitle>
        </DialogHeader>
        <div className="min-w-0 px-5 py-4">
          <JsonArea value={json} onChange={setJson} />
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

export function McpServerRow({ server, onRemove, onConfigure }) {
  const [open, setOpen] = useState(false);
  const [tools, setTools] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const id = server.id || server.name;
  const st = server.status || { state: 'unknown' };
  const connected = st.state === 'connected';
  const { label, tone, busy } = statusBadge(st);

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
    <div data-setting-row className="overflow-hidden">
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
        <Badge variant="outline" className={cn('h-5 gap-1 text-[10px]', tone)}>
          {busy && <Loader2 className="size-2.5 animate-spin" />}
          {label}
        </Badge>
        <Button
          size="sm" variant="ghost" className="h-7 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground"
          title="Configure per project"
          onClick={(e) => { e.stopPropagation(); onConfigure(server); }}
        >
          <Settings2 className="size-3.5" /> Configure
        </Button>
        <Button
          size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-destructive"
          onClick={(e) => { e.stopPropagation(); onRemove(id); }}
        >
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      {st.state === 'failed' && st.error && (
        <p className="px-3 pb-2 text-[11px] text-danger/90 break-all">{st.error}</p>
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
    </div>
  );
}

export function McpSection() {
  // `null` = not loaded yet (renders a skeleton, not the empty state).
  const [servers, setServers] = useState(null);
  const [jsonOpen, setJsonOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [configuring, setConfiguring] = useState(null);

  // The Settings list shows the global pool (projectId: null → pool only).
  // Per-project overrides are edited through each row's Configure dialog.
  // The call returns at once; servers still coming up arrive as `connecting`
  // and each row is filled in by its status event.
  const refresh = async () => {
    if (!isTauri()) { setServers([]); return; }
    try {
      // Listen before listing, so a server that settles instantly isn't missed.
      await mcpStatusReady();
      const list = await invoke('list_mcp_servers', { projectId: null });
      setServers(reconcileMcpList(Array.isArray(list) ? list : []));
    } catch { setServers((s) => s ?? []); }
  };
  useEffect(() => { refresh(); }, []);
  useExtensionsChanged(refresh);
  useMcpServerStatus((evt) => setServers((list) => applyMcpStatus(list, evt)));

  const remove = async (id) => {
    try { await invoke('remove_mcp_server', { id }); refresh(); }
    catch (e) { toastError(null, e); }
  };

  return (
    <Section
      title="MCP Servers"
      badge="Global"
      actions={
        <>
          <Button
            size="sm" variant="outline" className="h-7 text-xs gap-1.5"
            title="Edit the global pool of MCP servers"
            onClick={() => setJsonOpen(true)}
          >
            <ClipboardEdit className="size-3" /> Edit JSON
          </Button>
          <Button
            size="icon-sm" variant="ghost" className="size-7"
            title="Add an MCP server"
            onClick={() => setAddOpen(true)}
          >
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {servers === null ? (
        <div className="space-y-2" aria-busy="true" aria-label="Loading MCP servers">
          {[0, 1, 2].map((i) => (
            <div key={i} className="flex items-center gap-2 px-3 py-2">
              <Skeleton className="h-3.5 w-36" />
              <span className="flex-1" />
              <Skeleton className="h-5 w-24" />
            </div>
          ))}
        </div>
      ) : servers.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">
          No MCP servers configured.<br />
          Click + to add one, or "Edit JSON" to paste a standard <code className="text-[11px]">.mcp.json</code>.
        </div>
      ) : (
        <RowGroup inset>
          {servers.map((s) => (
            <McpServerRow key={s.id || s.name} server={s} onRemove={remove} onConfigure={setConfiguring} />
          ))}
        </RowGroup>
      )}
      <McpJsonDialog open={jsonOpen} onClose={() => { setJsonOpen(false); refresh(); }} />
      <McpAddServerDialog open={addOpen} onClose={() => { setAddOpen(false); refresh(); }} />
      <McpConfigureDialog
        server={configuring}
        open={!!configuring}
        onClose={() => { setConfiguring(null); refresh(); }}
      />
    </Section>
  );
}

