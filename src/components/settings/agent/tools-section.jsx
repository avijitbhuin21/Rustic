// Built-in tools section: web search + media tool dialogs, per-tool toggles.
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
import { Section, isTauri, slugify, useAiConfig } from './shared';
import { SubAgentSection } from './subagent-section';

// ─── Tools ───────────────────────────────────────────────────────────────────

export const WEB_SEARCH_BACKENDS = ['Tavily', 'Brave', 'Mcp'];

export function WebSearchDialog({ open, onClose, value, providers, onSave }) {
  const [backend, setBackend] = useState(value?.backend || 'Tavily');
  const [apiKey, setApiKey] = useState(value?.api_key || '');

  useEffect(() => {
    if (open) {
      setBackend(value?.backend || 'Tavily');
      setApiKey(value?.api_key || '');
    }
  }, [open, value]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[460px] sm:max-w-[460px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Web Search backend</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          <p className="text-[11px] italic text-muted-foreground leading-snug">
            Used when the active provider can't run web_search server-side (OpenAI Chat Completions, OpenAI-compatible, OpenRouter).
            Anthropic, Gemini, and GPT-5 already run it server-side — no key needed there.
          </p>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Backend</div>
            <Select value={backend} onValueChange={setBackend}>
              <SelectTrigger className="h-8 text-xs"><SelectValue /></SelectTrigger>
              <SelectContent>
                {WEB_SEARCH_BACKENDS.map((b) => <SelectItem key={b} value={b}>{b}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>
          {backend !== 'Mcp' && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">{backend} API key</div>
              <Input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                className="h-8 text-xs"
              />
            </div>
          )}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={() => { onSave({ backend, api_key: apiKey }); onClose(); }}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function MediaToolDialog({ open, onClose, title, badge, hint, providers, maxLimit, value, onSave }) {
  const [providerKey, setProviderKey] = useState('');
  const [model, setModel] = useState('');
  const [maxPerCall, setMaxPerCall] = useState(1);
  // Provider's model catalog. null = not loaded, [] = loaded empty / fetch
  // failed, array = ready. Re-fetched whenever the selected provider changes.
  const [providerModels, setProviderModels] = useState(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  useEffect(() => {
    if (open) {
      setProviderKey(value?.provider_key || '');
      setModel(value?.model || '');
      setMaxPerCall(value?.max_per_call || 1);
    }
  }, [open, value]);

  // Mirror the SubAgentSection model-loading pattern so the user picks from a
  // real model list instead of pasting a raw model id.
  useEffect(() => {
    let cancelled = false;
    const found = providers.find((p) => p.key === providerKey);
    if (!isTauri() || !found || !found.providerType) {
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
          // Media tools need image / video / audio model ids that the
          // chat-only filter would drop (e.g. Gemini VEO, Imagen, Nano Banana
          // variants that don't expose generateContent).
          includeAll: true,
        });
        if (!cancelled) setProviderModels(Array.isArray(list) ? list : []);
      } catch {
        if (!cancelled) setProviderModels([]);
      } finally {
        if (!cancelled) setModelsLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [providerKey, providers]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px] flex items-center gap-2">
            {title}
            <Badge variant="outline" className="h-5 text-[10px] font-mono">{badge}</Badge>
          </DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          {hint && <p className="text-[11px] italic text-muted-foreground leading-snug">{hint}</p>}
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Provider</div>
            <Select value={providerKey} onValueChange={(v) => { setProviderKey(v); setModel(''); }}>
              <SelectTrigger className="h-8 text-xs"><SelectValue placeholder="Pick a provider…" /></SelectTrigger>
              <SelectContent>
                {providers.length === 0 ? (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">No providers configured.</div>
                ) : providers.map((p) => (
                  <SelectItem key={p.key} value={p.key}>{p.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Model</div>
            <Select value={model} onValueChange={setModel} disabled={!providerKey}>
              <SelectTrigger className="h-8 text-xs">
                <SelectValue placeholder={
                  !providerKey
                    ? 'Pick a provider first'
                    : modelsLoading
                      ? 'Loading models…'
                      : 'Pick a model…'
                } />
              </SelectTrigger>
              <SelectContent>
                {/* Surface the saved model even when it isn't in the fetched
                    list (custom id, fetch failed) so the trigger isn't blank. */}
                {model && !(providerModels || []).includes(model) && (
                  <SelectItem key={model} value={model}>{model}</SelectItem>
                )}
                {(providerModels || []).map((m) => (
                  <SelectItem key={m} value={m}>{m}</SelectItem>
                ))}
                {providerKey && !modelsLoading && (providerModels || []).length === 0 && !model && (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">No models returned.</div>
                )}
              </SelectContent>
            </Select>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Max per call (1–{maxLimit})</div>
            <Input
              type="number" min={1} max={maxLimit}
              value={maxPerCall}
              onChange={(e) => setMaxPerCall(parseInt(e.target.value, 10) || 1)}
              disabled={!providerKey}
              className="h-8 text-xs w-24"
            />
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60 flex-row justify-between sm:justify-between">
          <Button
            variant="ghost" size="sm" className="text-xs text-muted-foreground hover:text-destructive"
            onClick={() => { onSave({ provider_key: '', model: '', max_per_call: 1 }); onClose(); }}
          >
            Disable tool
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
            <Button
              size="sm" className="text-xs"
              onClick={() => { onSave({ provider_key: providerKey, model: model.trim(), max_per_call: maxPerCall }); onClose(); }}
            >
              Save
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function ToolRow({ name, enabled, summary, statusLabel, onToggle, onConfigure, configurable = true }) {
  return (
    <div className="flex items-center gap-3 px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-[13px] font-medium">{name}</span>
          {statusLabel && (
            <span className="text-[11px] text-muted-foreground">{statusLabel}</span>
          )}
        </div>
        {summary && <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-1">{summary}</div>}
      </div>
      {configurable && (
        <Button
          size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-foreground"
          onClick={onConfigure} title="Configure"
        >
          <Pencil className="size-3.5" />
        </Button>
      )}
      {onToggle && <Switch checked={enabled} onCheckedChange={onToggle} />}
    </div>
  );
}

export function ToolsSection() {
  // Honor the one-shot deep-link from the prompt-box "Tool settings…" entry —
  // when settings was opened with section='tools', we render this section
  // already expanded. SettingsPanel clears the hint right after first mount,
  // so the user's manual collapses stick on subsequent visits.
  const [defaultOpen] = useState(
    () => useLayout.getState().settingsInitialSection === 'tools',
  );
  const wrapperRef = useRef(null);
  const { aiConfig } = useAiConfig();
  const [tool, setTool] = useState(null);
  const [wsOpen, setWsOpen] = useState(false);
  const [openMedia, setOpenMedia] = useState(null); // 'image' | 'video' | 'animate' | null

  // When opened via the deep-link, slide the section into view. The tab-switch
  // slideVariants animation takes ~200ms, so we wait a tick for layout to
  // settle before scrolling — otherwise the target's position is still being
  // animated and the scroll lands short.
  useEffect(() => {
    if (!defaultOpen) return;
    const t = setTimeout(() => {
      wrapperRef.current?.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }, 260);
    return () => clearTimeout(t);
  }, [defaultOpen]);

  const refreshTool = async () => {
    if (!isTauri()) return;
    try { setTool(await invoke('get_tool_config')); }
    catch {}
  };
  useEffect(() => { refreshTool(); }, []);

  const providers = (aiConfig?.providers || []).map((p) => {
    const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
    const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
    return {
      key,
      label,
      providerType: p.provider_type,
      baseUrl: p.base_url || null,
    };
  });

  const update = async (patch) => {
    if (!tool) return;
    const next = { ...tool, ...patch };
    setTool(next);
    try { await invoke('set_tool_config', { config: next }); }
    catch (e) { toast.error(String(e)); refreshTool(); }
  };

  if (!tool) return (
    <div ref={wrapperRef}>
      <Section title="Tools" defaultOpen={defaultOpen}>
        <div className="text-xs text-muted-foreground">Loading…</div>
      </Section>
    </div>
  );

  const ws = tool.web_search || { enabled: false, backend: 'Tavily', api_key: '' };
  const wf = tool.web_fetch || { enabled: true };
  const media = tool.media || { image: {}, video: {}, animate: {}, link_animate_to_video: false };

  const mediaStatus = (m) => m?.provider_key && m?.model ? `${m.provider_key} · ${m.model}` : 'Not configured';
  const animateEffective = media.link_animate_to_video ? media.video : media.animate;

  return (
    <div ref={wrapperRef}>
    <Section title="Tools" defaultOpen={defaultOpen}>
      <div className="rounded-lg border border-border/40 bg-muted/10 divide-y divide-border/40">
        <ToolRow
          name="Web Search"
          enabled={ws.enabled}
          summary={`Backend: ${ws.backend || 'Tavily'}${ws.backend !== 'Mcp' && !ws.api_key ? ' · no key' : ''}`}
          onToggle={(v) => update({ web_search: { ...ws, enabled: v } })}
          onConfigure={() => setWsOpen(true)}
        />
        <ToolRow
          name="Web Fetch"
          enabled={wf.enabled}
          summary="Fetch and summarize a URL"
          onToggle={(v) => update({ web_fetch: { enabled: v } })}
          configurable={false}
        />
        <ToolRow
          name="Image creator"
          statusLabel="image_create"
          summary={mediaStatus(media.image)}
          onConfigure={() => setOpenMedia('image')}
        />
        <ToolRow
          name="Video creator"
          statusLabel="video_create"
          summary={mediaStatus(media.video)}
          onConfigure={() => setOpenMedia('video')}
        />
        <div className="px-3 py-2.5">
          <div className="flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="text-[13px] font-medium">Animator</span>
                <span className="text-[11px] text-muted-foreground">animate</span>
              </div>
              <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-1">
                {media.link_animate_to_video
                  ? `Linked to Video creator · ${mediaStatus(media.video)}`
                  : mediaStatus(media.animate)}
              </div>
            </div>
            <label className="flex items-center gap-1.5 text-[11px] text-muted-foreground select-none cursor-pointer">
              <Switch
                checked={!!media.link_animate_to_video}
                onCheckedChange={(v) => update({ media: { ...media, link_animate_to_video: v } })}
              />
              link to video
            </label>
            {!media.link_animate_to_video && (
              <Button
                size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-foreground"
                onClick={() => setOpenMedia('animate')} title="Configure"
              >
                <Pencil className="size-3.5" />
              </Button>
            )}
          </div>
        </div>
      </div>

      <p className="mt-2 px-1 text-[11px] text-muted-foreground/80">
        Media outputs save under <code className="text-[11px]">.rustic/generated/</code>.
      </p>

      <WebSearchDialog
        open={wsOpen}
        onClose={() => setWsOpen(false)}
        value={ws}
        providers={providers}
        onSave={(v) => update({ web_search: { ...ws, ...v } })}
      />
      <MediaToolDialog
        open={openMedia === 'image'}
        onClose={() => setOpenMedia(null)}
        title="Image creator"
        badge="image_create"
        hint="Suggested: OpenAI gpt-image-1 · Gemini gemini-2.5-flash-image"
        providers={providers}
        maxLimit={10}
        value={media.image}
        onSave={(v) => update({ media: { ...media, image: v } })}
      />
      <MediaToolDialog
        open={openMedia === 'video'}
        onClose={() => setOpenMedia(null)}
        title="Video creator"
        badge="video_create"
        hint="Suggested: OpenAI sora-2 · Gemini veo-3.1-generate-preview. Veo 3.1 also enables first+last frame interpolation."
        providers={providers}
        maxLimit={4}
        value={media.video}
        onSave={(v) => update({ media: { ...media, video: v } })}
      />
      <MediaToolDialog
        open={openMedia === 'animate'}
        onClose={() => setOpenMedia(null)}
        title="Animator"
        badge="animate"
        hint="Animates an existing project image into a short clip. With Veo 3.1 you can also pass an end frame to interpolate between two images."
        providers={providers}
        maxLimit={4}
        value={media.animate}
        onSave={(v) => update({ media: { ...media, animate: v } })}
      />
    </Section>
    </div>
  );
}

