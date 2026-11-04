import React, { useEffect, useState, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Badge } from '@/components/ui/badge';
import { Separator } from '@/components/ui/separator';
import { Skeleton } from '@/components/ui/skeleton';
import { Save, RefreshCw, Check, X, Play, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { toastError } from '@/lib/toast-error';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';

// Last-loaded panel data per project, so reopening the panel renders instantly
// while a background refresh runs (list_mcp_servers connects servers and is slow).
const panelCache = new Map();

/** Merge fresh panel data into the per-project cache. */
function cachePatch(projectId, patch) {
  panelCache.set(projectId, { ...panelCache.get(projectId), ...patch });
}

export function McpPanel() {
  const projectId = useAgent((s) => s.activeProject.id);
  const cached = panelCache.get(projectId);
  // `null` = not loaded yet (renders a skeleton instead of misleading defaults).
  const [json, setJson] = useState(cached?.json ?? null);
  const [servers, setServers] = useState(cached?.servers ?? null);
  // `pending` is the single pending-consent object (or null) returned by
  // `get_pending_mcp_consent` — it carries the contentHash the user is
  // approving so we can pass it back unchanged.
  const [pending, setPending] = useState(cached?.pending ?? null);
  const [loading, setLoading] = useState(false);
  const projectRef = useRef(projectId);
  // Unsaved edits in the textarea must survive a background refresh.
  const jsonDirty = useRef(false);

  useEffect(() => {
    projectRef.current = projectId;
    jsonDirty.current = false;
    const c = panelCache.get(projectId);
    setJson(c?.json ?? null);
    setServers(c?.servers ?? null);
    setPending(c?.pending ?? null);
  }, [projectId]);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    const pid = projectId;
    const current = () => projectRef.current === pid;
    setLoading(true);
    const jsonTask = invoke('read_mcp_json', { scope: 'project', projectId: pid })
      .then((raw) => {
        if (typeof raw !== 'string') return;
        cachePatch(pid, { json: raw });
        if (current() && !jsonDirty.current) setJson(raw);
      })
      .catch(() => {
        if (current()) setJson((j) => j ?? '{\n  "mcpServers": {}\n}');
      });
    const listTask = invoke('list_mcp_servers', { projectId: pid })
      .then((list) => {
        const next = Array.isArray(list) ? list : [];
        cachePatch(pid, { servers: next });
        if (current()) setServers(next);
      })
      .catch(() => {
        if (current()) setServers((s) => s ?? []);
      });
    const pendingTask = invoke('get_pending_mcp_consent', { projectId: pid })
      .then((pend) => {
        cachePatch(pid, { pending: pend ?? null });
        if (current()) setPending(pend ?? null);
      })
      .catch(() => {
        if (current()) setPending(null);
      });
    await Promise.allSettled([jsonTask, listTask, pendingTask]);
    if (current()) setLoading(false);
  }, [projectId]);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten = null;
    let disposed = false;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('mcp-consent-required', () => load()))
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [load]);

  const save = async () => {
    if (!isTauri() || json === null) return;
    try {
      JSON.parse(json);
    } catch (e) {
      toast.error('Invalid JSON');
      return;
    }
    try {
      await invoke('save_mcp_json', { scope: 'project', projectId, content: json });
      jsonDirty.current = false;
      cachePatch(projectId, { json });
      toast.success('MCP config saved');
      load();
    } catch (e) {
      toast.error('Save failed');
    }
  };

  const approve = async () => {
    if (!isTauri() || !pending) return;
    const contentHash = pending.contentHash || pending.content_hash;
    if (!contentHash) {
      toast.error('Missing content hash');
      return;
    }
    try {
      await invoke('approve_mcp_project_consent', { projectId, contentHash });
      load();
    } catch (e) {
      toastError(null, e);
    }
  };

  const revoke = async () => {
    if (!isTauri()) return;
    try {
      await invoke('revoke_mcp_project_consent', { projectId });
      load();
    } catch (e) {
      toastError(null, e);
    }
  };

  const test = async (serverId) => {
    if (!isTauri()) return;
    try {
      const ok = await invoke('test_mcp_server', { id: serverId });
      toast[ok ? 'success' : 'error'](`Test: ${ok ? 'OK' : 'failed'}`);
    } catch (e) {
      toast.error('Test error');
    }
  };

  const remove = async (serverId) => {
    if (!isTauri()) return;
    try {
      await invoke('remove_mcp_server', { id: serverId });
      load();
    } catch (e) { toastError('Remove failed', e); }
  };

  return (
    <ScrollArea className="h-full min-h-0 flex-1">
      <div className="space-y-4 p-3">
        <section>
          <div className="mb-1.5 flex items-center justify-between">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              MCP Servers
            </h3>
            <Button
              size="sm"
              variant="ghost"
              className="h-6 px-1.5 text-xs"
              onClick={load}
              disabled={loading}
              title="Refresh"
            >
              <RefreshCw className={cn('size-3', loading && 'animate-spin')} />
            </Button>
          </div>
          {servers === null ? (
            <ul className="space-y-1" aria-busy="true" aria-label="Loading MCP servers">
              {[0, 1, 2].map((i) => (
                <li
                  key={i}
                  className="flex items-center gap-1.5 rounded border border-border bg-muted/30 px-2 py-1.5"
                >
                  <Skeleton className="h-3.5 w-28" />
                  <Skeleton className="h-3.5 w-14" />
                  <div className="ml-auto flex items-center gap-1">
                    <Skeleton className="size-5" />
                    <Skeleton className="size-5" />
                  </div>
                </li>
              ))}
            </ul>
          ) : servers.length === 0 ? (
            <div className="text-xs text-muted-foreground">No servers configured.</div>
          ) : (
            <ul className="space-y-1">
              {servers.map((s) => (
                <li
                  key={s.id || s.name}
                  className="flex items-center gap-1.5 rounded border border-border bg-muted/30 px-2 py-1.5 text-xs"
                >
                  <span className="font-mono">{s.name || s.id}</span>
                  {s.status && (
                    <Badge variant="outline" className="h-4 text-[10px]">
                      {(typeof s.status === 'string' ? s.status : s.status.state) ?? 'unknown'}
                      {typeof s.status === 'object' && s.status.tool_count != null
                        ? ` · ${s.status.tool_count}`
                        : ''}
                    </Badge>
                  )}
                  <div className="ml-auto flex items-center gap-1">
                    <Button
                      size="icon"
                      variant="ghost"
                      className="size-6"
                      title="Test"
                      onClick={() => test(s.id || s.name)}
                    >
                      <Play className="size-3" />
                    </Button>
                    <Button
                      size="icon"
                      variant="ghost"
                      className="size-6"
                      title="Remove"
                      onClick={() => remove(s.id || s.name)}
                    >
                      <Trash2 className="size-3" />
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          <div className="mt-2">
            <Button
              size="sm"
              variant="ghost"
              className="h-6 px-2 text-[11px] text-muted-foreground"
              onClick={revoke}
            >
              Revoke MCP consent for project
            </Button>
          </div>
        </section>

        {pending && (
          <section>
            <h3 className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              Pending consent
            </h3>
            <div className="flex items-center gap-1.5 rounded border border-warning/30 bg-warning/5 px-2 py-1.5 text-xs">
              <span className="font-mono truncate">{pending.projectPath || pending.project_path || 'project'}</span>
              <div className="ml-auto flex items-center gap-1">
                <Button
                  size="sm"
                  variant="outline"
                  className="h-6 px-1.5 text-xs"
                  onClick={approve}
                  title="Approve project MCP servers"
                >
                  <Check className="size-3" />
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-1.5 text-xs"
                  onClick={revoke}
                  title="Revoke MCP consent for project"
                >
                  <X className="size-3" />
                </Button>
              </div>
            </div>
          </section>
        )}

        <Separator />

        <section>
          <div className="mb-1.5 flex items-center justify-between">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              mcp.json
            </h3>
            <Button
              size="sm"
              variant="default"
              className="h-6 px-2 text-xs"
              onClick={save}
              disabled={json === null}
            >
              <Save className="mr-1 size-3" /> Save
            </Button>
          </div>
          {json === null ? (
            <Skeleton className="h-[220px] w-full" aria-busy="true" aria-label="Loading mcp.json" />
          ) : (
            <Textarea
              value={json}
              onChange={(e) => {
                jsonDirty.current = true;
                setJson(e.target.value);
              }}
              className="min-h-[220px] font-mono text-[11px]"
              spellCheck={false}
            />
          )}
        </section>
      </div>
    </ScrollArea>
  );
}

export default McpPanel;
