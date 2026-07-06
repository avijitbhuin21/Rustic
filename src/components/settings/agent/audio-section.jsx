// Audio-input (voice transcription) model section.
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

// ─── Audio Input ───────────────────────────────────────────────────────────────

export function AudioInputSection() {
  const { aiConfig: config, refreshAiConfig } = useAiConfig();
  const [providerKey, setProviderKey] = useState('');
  const [model, setModel] = useState('');
  const [providerModels, setProviderModels] = useState(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  useEffect(() => {
    if (!config) return;
    if (config.audio_input) {
      setProviderKey(config.audio_input.provider_key || '');
      setModel(config.audio_input.model || '');
    } else {
      setProviderKey(''); setModel('');
    }
  }, [config]);

  // OpenAI / Compatible transcribe via /audio/transcriptions (Whisper),
  // Gemini via generateContent, OpenRouter via chat/completions input_audio —
  // the backend routes per provider type, so all four are selectable here.
  // (Anthropic has no transcription path and is intentionally excluded.)
  const providers = (config?.providers || [])
    .filter((p) => ['OpenAi', 'Compatible', 'Gemini', 'OpenRouter'].includes(p.provider_type))
    .map((p) => {
      const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
      const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
      return { key, label, providerType: p.provider_type, baseUrl: p.base_url || null };
    });

  // Load models with includeAll so transcription models (whisper-1,
  // gpt-4o-transcribe) — which the chat-only filter hides — are selectable.
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerKey, config]);

  const save = async () => {
    try {
      if (providerKey && model.trim()) {
        await invoke('set_audio_input_config', { providerKey, model: model.trim() });
        toast.success('Audio input saved');
      } else {
        await invoke('clear_audio_input_config');
        toast.success('Audio input disabled');
      }
      refreshAiConfig();
      // Tell the prompt-box(es) to re-read the audio flag so the mic toggles
      // immediately — Settings is an in-app panel, so no window focus fires.
      window.dispatchEvent(new Event('audio-input-changed'));
    } catch (e) { toast.error(String(e)); }
  };

  const clearChoice = async () => {
    try {
      await invoke('clear_audio_input_config');
      setProviderKey(''); setModel('');
      refreshAiConfig();
      window.dispatchEvent(new Event('audio-input-changed'));
    } catch (e) { toast.error(String(e)); }
  };

  return (
    <Section title="Audio Input">
      <p className="mb-3 text-[12px] italic leading-snug text-muted-foreground">
        Pick a speech-to-text model and a mic button appears in the chat composer: click it (when the box is empty) to
        record, and your speech is transcribed straight into the prompt. Works with OpenAI &amp; OpenAI-compatible
        Whisper models (<code className="text-[11px]">gpt-4o-transcribe</code>, <code className="text-[11px]">whisper-1</code>),
        Gemini, and any audio-capable OpenRouter model — pick any model from those providers. Transcript quality and
        whether it streams word-by-word depend on the model. Leave unset to hide the mic.
      </p>
      <div className="flex items-center gap-2">
        <Select value={providerKey} onValueChange={(v) => { setProviderKey(v); setModel(''); }}>
          <SelectTrigger className="h-8 w-40 text-xs">
            <SelectValue placeholder="Pick a provider…" />
          </SelectTrigger>
          <SelectContent>
            {providers.length === 0 && (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">Connect an OpenAI, Gemini, OpenRouter or Compatible provider first.</div>
            )}
            {providers.map((p) => (
              <SelectItem key={p.key} value={p.key}>{p.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={model} onValueChange={setModel} disabled={!providerKey}>
          <SelectTrigger className="h-8 flex-1 text-xs">
            <SelectValue placeholder={
              !providerKey ? 'Pick a provider first' : modelsLoading ? 'Loading models…' : 'Pick a model…'
            } />
          </SelectTrigger>
          <SelectContent>
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
        {(providerKey || model) && (
          <Button size="icon-sm" variant="ghost" className="size-8 text-muted-foreground hover:text-destructive" onClick={clearChoice}>
            <Trash2 className="size-3.5" />
          </Button>
        )}
      </div>

      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save}>Save</Button>
      </div>
    </Section>
  );
}

