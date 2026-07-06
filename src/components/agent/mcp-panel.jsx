import React, { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Badge } from '@/components/ui/badge';
import { Separator } from '@/components/ui/separator';
import { Save, RefreshCw, Check, X, Play, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useAgent } from '@/state/agent';


export function McpPanel() {
  const projectId = useAgent((s) => s.activeProject.id);
  const [json, setJson] = useState('{\n  "mcpServers": {}\n}');
  const [servers, setServers] = useState([]);
  // `pending` is the single pending-consent object (or null) returned by
  // `get_pending_mcp_consent` — it carries the contentHash the user is
  // approving so we can pass it back unchanged.
  const [pending, setPending] = useState(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    try {
      const raw = await invoke('read_mcp_json', { scope: 'project', projectId });
      if (typeof raw === 'string') setJson(raw);
    } catch (e) {}
    try {
      const list = await invoke('list_mcp_servers', { projectId });
      setServers(Array.isArray(list) ? list : []);
    } catch (e) {}
    try {
      const pend = await invoke('get_pending_mcp_consent', { projectId });
      setPending(pend ?? null);
    } catch (e) {
      setPending(null);
    }
    setLoading(false);
  }, [projectId]);

  useEffect(() => {
    load();
  }, [load]);

  const save = async () => {
    if (!isTauri()) return;
    try {
      JSON.parse(json);
    } catch (e) {
      toast.error('Invalid JSON');
      return;
    }
    try {
      await invoke('save_mcp_json', { scope: 'project', projectId, content: json });
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
      toast.error(String(e));
    }
  };

  const revoke = async () => {
    if (!isTauri()) return;
    try {
      await invoke('revoke_mcp_project_consent', { projectId });
      load();
    } catch (e) {
      toast.error(String(e));
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
    } catch (e) {}
  };

  return (
    <ScrollArea className="h-full">
      <div className="space-y-4 p-3">
        <section>
          <div className="mb-1.5 flex items-center justify-between">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              MCP Servers
            </h3>
            <Button size="sm" variant="ghost" className="h-6 px-1.5 text-xs" onClick={load}>
              <RefreshCw className="size-3" />
            </Button>
          </div>
          {servers.length === 0 ? (
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
            <div className="flex items-center gap-1.5 rounded border border-amber-500/30 bg-amber-500/5 px-2 py-1.5 text-xs">
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
            <Button size="sm" variant="default" className="h-6 px-2 text-xs" onClick={save} disabled={loading}>
              <Save className="mr-1 size-3" /> Save
            </Button>
          </div>
          <Textarea
            value={json}
            onChange={(e) => setJson(e.target.value)}
            className="min-h-[220px] font-mono text-[11px]"
            spellCheck={false}
          />
        </section>
      </div>
    </ScrollArea>
  );
}

export default McpPanel;
