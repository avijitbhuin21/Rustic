// Sub-agent model preference section.
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
import { SearchableSelect } from '@/components/ui/searchable-select';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, isTauri, slugify, useAiConfig } from './shared';

// ─── Sub Agent ───────────────────────────────────────────────────────────────

export function SubAgentSection() {
  const { aiConfig: config } = useAiConfig();
  const [providerKey, setProviderKey] = useState('');
  const [model, setModel] = useState('');
  const [capEnabled, setCapEnabled] = useState(true);
  const [cap, setCap] = useState(10);
  // Models fetched for the currently selected provider. null = not loaded yet,
  // [] = loaded but empty (or fetch failed), array = ready.
  const [providerModels, setProviderModels] = useState(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  // Track subagent + budget from the shared aiConfig snapshot. Providers come
  // from the shared context (kept fresh across sections); subagent/budget are
  // also part of that config so we just sync them locally on each refresh.
  useEffect(() => {
    if (!config) return;
    if (config.subagent) {
      setProviderKey(config.subagent.provider_key || '');
      setModel(config.subagent.model || '');
    } else {
      setProviderKey(''); setModel('');
    }
    const c = config.budget?.max_concurrent_subagents;
    if (c === null || c === undefined) { setCapEnabled(true); setCap(10); }
    else { setCapEnabled(true); setCap(c); }
  }, [config]);

  const { refreshAiConfig } = useAiConfig();
  const refresh = refreshAiConfig;

  const providers = (config?.providers || []).map((p) => {
    const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
    const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
    return {
      key,
      label,
      defaultModel: p.default_model,
      providerType: p.provider_type,
      baseUrl: p.base_url || null,
      binaryPath: p.binary_path || null,
    };
  });

  // Load the model list for whichever provider is currently selected.
  useEffect(() => {
    let cancelled = false;
    const found = providers.find((p) => p.key === providerKey);
    if (!isTauri() || !found) {
      setProviderModels(null);
      return () => { cancelled = true; };
    }
    setModelsLoading(true);
    (async () => {
      try {
        const list = await invoke('fetch_ai_models', {
          providerType: found.providerType,
          apiKey: '__STORED__',
          baseUrl: found.baseUrl || null,
          forceRefresh: false,
          includeAll: false,
        });
        if (!cancelled) setProviderModels(Array.isArray(list) ? list : []);
      } catch {
        if (!cancelled) setProviderModels([]);
      } finally {
        if (!cancelled) setModelsLoading(false);
      }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerKey, config]);

  const onPick = (key) => {
    setProviderKey(key);
    const found = providers.find((p) => p.key === key);
    // Pre-fill with the provider's default model so users who don't want to
    // dig through the dropdown still get a sane pick.
    setModel(found?.defaultModel || '');
  };

  const save = async () => {
    try {
      if (providerKey && model.trim()) {
        await invoke('set_subagent_config', { providerKey, model: model.trim() });
      } else {
        await invoke('clear_subagent_config');
      }
      await invoke('set_subagent_concurrency_cap', { cap: capEnabled ? Number(cap) : null });
      toast.success('Sub-agent saved');
      refresh();
    } catch (e) { toast.error(String(e)); }
  };

  const clearChoice = async () => {
    try {
      await invoke('clear_subagent_config');
      refresh();
    } catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="Sub Agent"
      info="Pick a cheaper, faster model the agent can route mechanical sub-agent work to. When set, the main agent picks per-spawn whether the sub-agent runs on the main chat model (best for reasoning) or this one (best for bulk reads, simple edits, summarising). Leave unset to always reuse the main model."
    >
      <div className="flex items-center gap-2">
        <Select value={providerKey} onValueChange={onPick}>
          <SelectTrigger className="h-8 w-40 text-xs">
            <SelectValue placeholder="Pick a provider…" />
          </SelectTrigger>
          <SelectContent>
            {providers.length === 0 && (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">No providers configured.</div>
            )}
            {providers.map((p) => (
              <SelectItem key={p.key} value={p.key}>{p.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <SearchableSelect
          value={model}
          onValueChange={setModel}
          disabled={!providerKey}
          options={providerModels || []}
          className="flex-1"
          placeholder={
            !providerKey
              ? 'Pick a provider first'
              : modelsLoading
                ? 'Loading models…'
                : 'Pick a model…'
          }
          searchPlaceholder="Search models…"
          emptyText={modelsLoading ? 'Loading models…' : 'No models returned.'}
        />
        {(providerKey || model) && (
          <Button size="icon-sm" variant="ghost" className="size-8 text-muted-foreground hover:text-destructive" onClick={clearChoice}>
            <Trash2 className="size-3.5" />
          </Button>
        )}
      </div>

      <div className="mt-5">
        <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-2">Concurrency</div>
        <div className="-mx-4 border-y border-border/40 px-4 py-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5 text-[13px] font-medium">
                Cap parallel sub-agents per task
                <InfoTip>
                  How many <code className="text-[11px]">spawn_subagent</code> calls can run simultaneously under one parent
                  task. Default 10. Uncheck to lift the cap entirely (rate-limit safety still comes from the global stream
                  cap in the Budget panel).
                </InfoTip>
              </div>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <Switch checked={capEnabled} onCheckedChange={setCapEnabled} />
              <Input
                type="number"
                min={1}
                max={64}
                value={cap}
                onChange={(e) => setCap(parseInt(e.target.value, 10) || 1)}
                disabled={!capEnabled}
                className="h-7 w-16 text-xs"
              />
              <span className="text-[11px] text-muted-foreground">sub-agents</span>
            </div>
          </div>
        </div>
      </div>

      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save}>Save</Button>
      </div>
    </Section>
  );
}

