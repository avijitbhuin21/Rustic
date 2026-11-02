// GitHub auto-resolve section.
// Split out of agent-settings.jsx (A4).
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronRight, ChevronDown, Plus, Eye, EyeOff, Pencil, Trash2, Info, RefreshCw,
  ClipboardEdit, X, Check, FileText, Copy, List, Loader2,
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
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, slugify, useAiConfig } from './shared';
import { RowGroup } from '../setting-row';
import { SearchableSelect } from '@/components/ui/searchable-select';

// ─── GitHub auto issue resolve (web/server build only) ──────────────────────

export function GithubAutoResolveSection() {
  const { aiConfig } = useAiConfig();
  const projects = useExplorer((s) => s.projects);
  const [cfg, setCfg] = useState(null); // { enabled, publicBaseUrl, label }
  const [signedIn, setSignedIn] = useState(false);
  const [projectId, setProjectId] = useState(null);
  const [projCfg, setProjCfg] = useState(null);
  const [detectedRepo, setDetectedRepo] = useState(null);
  const [savingProject, setSavingProject] = useState(false);

  const refreshGlobal = useCallback(async () => {
    try {
      const r = await invoke('github_auto_get_config');
      setCfg(r.config);
      setSignedIn(!!r.signedIn);
    } catch { /* server route missing — leave section in loading state */ }
  }, []);
  useEffect(() => { refreshGlobal(); }, [refreshGlobal]);

  useEffect(() => {
    if (!projectId) { setProjCfg(null); setDetectedRepo(null); return; }
    let active = true;
    invoke('github_auto_get_project_config', { projectId })
      .then((r) => { if (active) { setProjCfg(r.config); setDetectedRepo(r.detectedRepo); } })
      .catch((e) => { if (active) { setProjCfg(null); toast.error(String(e)); } });
    return () => { active = false; };
  }, [projectId]);

  const saveGlobal = async (next) => {
    try {
      const saved = await invoke('github_auto_set_config', {
        enabled: next.enabled,
        publicBaseUrl: next.publicBaseUrl ?? '',
        label: next.label || 'rustic',
      });
      setCfg(saved);
      toast.success('GitHub auto-resolve settings saved');
    } catch (e) { toast.error(String(e)); refreshGlobal(); }
  };

  const saveProject = async (next) => {
    if (!projectId) return;
    setSavingProject(true);
    try {
      const saved = await invoke('github_auto_set_project_config', {
        projectId,
        enabled: next.enabled,
        costCapUsd: next.costCapUsd ?? null,
        model: next.model ?? null,
        providerType: next.providerType ?? null,
      });
      setProjCfg(saved);
      if (next.enabled && !projCfg?.enabled) {
        toast.success(`Auto-resolve enabled — webhook created on ${saved.repoFullName || 'the repo'}`);
      } else {
        toast.success('Project settings saved');
      }
    } catch (e) { toast.error(String(e)); }
    finally { setSavingProject(false); }
  };

  const providers = (aiConfig?.providers || []).map((p) => {
    const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
    const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
    return { key, label, providerType: p.provider_type, baseUrl: p.base_url || null };
  });

  // Live model lists for the issue-task model dropdown — same cache the chat
  // model picker uses (backend caches /v1/models for 5 min on top).
  const liveByKey = useLiveModels((s) => s.byKey);
  const loadLive = useLiveModels((s) => s.load);
  useEffect(() => {
    if (!projectId || !aiConfig) return;
    for (const p of aiConfig.providers || []) {
      const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
      loadLive({ key, providerType: p.provider_type, baseUrl: p.base_url || null });
    }
  }, [projectId, aiConfig, loadLive]);

  if (!cfg) {
    return (
      <Section title="GitHub Auto-Resolve" badge="server">
        <div className="text-xs text-muted-foreground">Loading…</div>
      </Section>
    );
  }

  return (
    <Section
      title="GitHub Auto-Resolve"
      badge="server"
      info={(
        <>
          Issues labeled <span className="font-mono">{cfg.label || 'rustic'}</span> on connected
          repos are pulled into <span className="font-mono">issues/</span>, fixed by a dedicated
          agent task (queued one at a time), and committed locally — never pushed. Clarifying
          questions go back and forth as issue comments.
        </>
      )}
    >

      {!signedIn && (
        <div className="mb-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-[12px] text-warning">
          Sign in to GitHub (status bar, bottom left) first — the integration reuses that account
          to read issues, post comments and create webhooks.
        </div>
      )}

      <RowGroup inset>
        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 text-[13px] font-medium">
              Auto issue resolve
              <InfoTip>Master switch. Off = webhooks are ignored and the queue pauses.</InfoTip>
            </div>
          </div>
          <Switch
            checked={!!cfg.enabled}
            onCheckedChange={(v) => saveGlobal({ ...cfg, enabled: v })}
          />
        </div>

        <div className="px-3 py-3">
          <div className="mb-2 flex items-center gap-1.5 text-[13px] font-medium">
            Public server URL
            <InfoTip>Where GitHub delivers webhooks, e.g. <span className="font-mono">https://rustic.example.com</span>.</InfoTip>
          </div>
          <div className="flex items-center gap-2">
            <Input
              value={cfg.publicBaseUrl || ''}
              onChange={(e) => setCfg({ ...cfg, publicBaseUrl: e.target.value })}
              placeholder="https://your-server.example.com"
              className="h-7 flex-1 text-xs font-mono"
            />
            <Input
              value={cfg.label || ''}
              onChange={(e) => setCfg({ ...cfg, label: e.target.value })}
              placeholder="rustic"
              title="Only issues with this label are processed"
              className="h-7 w-28 text-xs font-mono"
            />
            <Button size="sm" className="text-xs" onClick={() => saveGlobal(cfg)}>Save</Button>
          </div>
        </div>

        <div className="px-3 py-3">
          <div className="text-[13px] font-medium mb-2">Per-project</div>
          <Select value={projectId ?? ''} onValueChange={setProjectId}>
            <SelectTrigger className="h-7 w-full text-xs">
              <SelectValue placeholder="Pick a project…" />
            </SelectTrigger>
            <SelectContent>
              {projects.map((p) => (
                <SelectItem key={p.id} value={p.id} className="text-xs">{p.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>

          {projectId && projCfg && (
            <div className="mt-3 space-y-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5 text-[12px] font-medium">
                    Enable for this project
                    <InfoTip>Enabling creates the repo webhook automatically (needs the public URL above).</InfoTip>
                    {detectedRepo && (
                      <span className="ml-0.5 font-mono text-[11px] text-muted-foreground">{detectedRepo}</span>
                    )}
                  </div>
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  {savingProject && <Loader2 className="size-3 animate-spin text-muted-foreground" />}
                  <Switch
                    checked={!!projCfg.enabled}
                    disabled={savingProject || (!detectedRepo && !projCfg.enabled)}
                    onCheckedChange={(v) => saveProject({ ...projCfg, enabled: v })}
                  />
                </div>
              </div>

              <div className="flex items-center gap-2">
                <span className="text-[12px] text-muted-foreground w-28 shrink-0">Cost cap per issue</span>
                <Input
                  type="number" min={0} step="0.5"
                  value={projCfg.costCapUsd ?? ''}
                  placeholder="uncapped"
                  onChange={(e) => setProjCfg({
                    ...projCfg,
                    costCapUsd: e.target.value === '' ? null : parseFloat(e.target.value),
                  })}
                  className="h-7 w-24 text-xs"
                />
                <span className="text-[11px] text-muted-foreground">
                  USD — each issue's fixer task may spend up to this, not the project as a whole
                </span>
              </div>

              <div className="flex items-center gap-2">
                <span className="text-[12px] text-muted-foreground w-28 shrink-0">Issue-task model</span>
                <SearchableSelect
                  className="h-7 flex-1"
                  value={
                    projCfg.providerType && projCfg.model
                      ? `${projCfg.providerType}::${projCfg.model}`
                      : '__default__'
                  }
                  onValueChange={(v) => {
                    if (v === '__default__') {
                      setProjCfg({ ...projCfg, providerType: null, model: null });
                    } else {
                      const sep = v.indexOf('::');
                      setProjCfg({
                        ...projCfg,
                        providerType: v.slice(0, sep),
                        model: v.slice(sep + 2),
                      });
                    }
                  }}
                  placeholder="Project default"
                  searchPlaceholder="Search models…"
                  groups={[
                    { label: null, options: [{ value: '__default__', label: 'Project default' }] },
                    // A previously-saved model stays selectable (SearchableSelect
                    // surfaces an unknown value) even when the live list lacks it.
                    ...providers
                      .map((p) => ({
                        label: p.label,
                        options: (liveByKey[p.key] || [])
                          .map((m) => m.id || m.model_id)
                          .filter(Boolean)
                          .map((id) => ({ value: `${p.key}::${id}`, label: id })),
                      }))
                      .filter((g) => g.options.length > 0),
                  ]}
                />
              </div>

              <div className="flex justify-end">
                <Button
                  size="sm" variant="outline" className="text-xs"
                  disabled={savingProject}
                  onClick={() => saveProject(projCfg)}
                >
                  Save project settings
                </Button>
              </div>
            </div>
          )}
        </div>
      </RowGroup>
    </Section>
  );
}

