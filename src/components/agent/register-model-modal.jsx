import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { useCustomModels } from '@/state/custom-models';
import { useAgent } from '@/state/agent';

// Modal prompting the user to fill in the cost / context-window specs for a
// model that isn't in the built-in registry. Mirrors the legacy JS flow:
// users can start fresh, copy specs from a user-saved template, or copy from
// any built-in registry model — keeping pricing accurate even for unfamiliar
// model ids (e.g. OpenRouter's full catalogue).
//
// On save:
//   - persists the spec to localStorage via useCustomModels.save()
//   - persists capability flags to the backend via set_model_capabilities
//   - calls onSaved() so the caller can switch the active model to this id

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

const PROVIDER_TYPES = ['Claude', 'OpenAi', 'Gemini', 'OpenRouter', 'Compatible'];

// Friendly labels shown next to the locked provider field — match the model
// picker's labels so the user sees the same name in both places.
const PROVIDER_LABELS = {
  Claude: 'Anthropic',
  OpenAi: 'OpenAI',
  Gemini: 'Google',
  OpenRouter: 'OpenRouter',
  Compatible: 'OpenAI-Compatible',
};

function NumberInput({ value, onChange, placeholder, step = '1' }) {
  return (
    <Input
      type="number"
      step={step}
      placeholder={placeholder}
      value={value ?? ''}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

export function RegisterModelModal({
  open,
  onOpenChange,
  modelId,
  providerType = null,
  onSaved,
}) {
  const customModels = useCustomModels((s) => s.models);
  const saveCustomModel = useCustomModels((s) => s.save);
  const builtins = useAgent((s) => s.models);

  const existing = modelId ? customModels[modelId] : null;
  const isEdit = !!existing;

  // Local form state. Re-initialised whenever the modal opens for a different
  // model so previous edits don't bleed into a new registration.
  const [name, setName] = useState('');
  const [provider, setProvider] = useState(providerType || 'Claude');
  const [contextWindow, setContextWindow] = useState('');
  const [maxOutput, setMaxOutput] = useState('');
  const [inputCost, setInputCost] = useState('');
  const [outputCost, setOutputCost] = useState('');
  const [cachedInCost, setCachedInCost] = useState('');
  const [cachedOutCost, setCachedOutCost] = useState('');
  const [sendsTemperature, setSendsTemperature] = useState(true);
  const [supportsReasoning, setSupportsReasoning] = useState(true);
  const [templateKey, setTemplateKey] = useState('');
  const [submitError, setSubmitError] = useState('');
  const [saving, setSaving] = useState(false);

  // Build the template options: user-saved entries (most-recent first), then
  // built-in registry grouped by provider. Each entry maps to the spec to
  // apply when chosen so the change handler can stay one-liner.
  const { templateOptions, specByKey } = useMemo(() => {
    const opts = [];
    const map = new Map();
    const userEntries = Object.entries(customModels)
      .filter(([id]) => id !== modelId)
      .sort(([, a], [, b]) => (b.savedAt || 0) - (a.savedAt || 0));
    if (userEntries.length > 0) {
      opts.push({ kind: 'group', label: 'Your saved templates', items: [] });
      const group = opts[opts.length - 1];
      for (const [id, spec] of userEntries) {
        const key = `user:${id}`;
        const display = spec.name && spec.name !== id ? `${spec.name} — ${id}` : id;
        const suffix = spec.provider ? ` (${spec.provider})` : '';
        group.items.push({ key, label: `${display}${suffix}` });
        map.set(key, spec);
      }
    }
    if (Array.isArray(builtins) && builtins.length > 0) {
      const byProvider = new Map();
      for (const m of builtins) {
        const p = m.provider || 'Other';
        if (!byProvider.has(p)) byProvider.set(p, []);
        byProvider.get(p).push(m);
      }
      for (const [p, models] of byProvider) {
        const group = { kind: 'group', label: PROVIDER_LABELS[p] || p, items: [] };
        for (const m of models) {
          const key = `builtin:${m.id}`;
          group.items.push({ key, label: `${m.name} — ${m.id}` });
          map.set(key, {
            contextWindow: m.context_window,
            maxOutputTokens: m.max_output_tokens,
            inputCost: m.input_cost_per_m,
            outputCost: m.output_cost_per_m,
            cachedInputCost: m.cache_read_cost_per_m,
            cachedOutputCost: m.cache_write_cost_per_m,
          });
        }
        opts.push(group);
      }
    }
    return { templateOptions: opts, specByKey: map };
  }, [customModels, builtins, modelId]);

  // Re-hydrate form state on open. The capability flags need an async pull
  // from the backend; do it inside the effect so we don't fire on every render.
  useEffect(() => {
    if (!open) return;
    setName(existing?.name || '');
    setProvider(providerType || existing?.provider || 'Claude');
    setContextWindow(existing?.contextWindow ?? '');
    setMaxOutput(existing?.maxOutputTokens ?? '');
    setInputCost(existing?.inputCost ?? '');
    setOutputCost(existing?.outputCost ?? '');
    setCachedInCost(existing?.cachedInputCost ?? '');
    setCachedOutCost(existing?.cachedOutputCost ?? '');
    setSendsTemperature(true);
    setSupportsReasoning(true);
    setTemplateKey('');
    setSubmitError('');

    if (isTauri() && modelId) {
      invoke('get_model_capabilities')
        .then((caps) => {
          const entry = caps && caps[modelId];
          if (entry && typeof entry.supports_temperature === 'boolean') {
            setSendsTemperature(entry.supports_temperature);
          }
          if (entry && typeof entry.supports_reasoning_effort === 'boolean') {
            setSupportsReasoning(entry.supports_reasoning_effort);
          }
        })
        .catch(() => {});
    }
  }, [open, modelId, providerType, existing]);

  const applyTemplate = (key) => {
    setTemplateKey(key);
    const spec = specByKey.get(key);
    if (!spec) return;
    setContextWindow(spec.contextWindow ?? '');
    setMaxOutput(spec.maxOutputTokens ?? '');
    setInputCost(spec.inputCost ?? '');
    setOutputCost(spec.outputCost ?? '');
    setCachedInCost(spec.cachedInputCost ?? '');
    setCachedOutCost(spec.cachedOutputCost ?? '');
    // Display Name and Provider are intentionally not overwritten — same model,
    // new naming / hosting.
  };

  const handleSave = async () => {
    setSubmitError('');
    const ctx = parseInt(contextWindow, 10);
    const mout = parseInt(maxOutput, 10);
    const ic = parseFloat(inputCost);
    const oc = parseFloat(outputCost);
    const cic = parseFloat(cachedInCost);
    const coc = parseFloat(cachedOutCost);

    if (!provider) {
      setSubmitError('Provider is required');
      return;
    }
    if (!Number.isFinite(ctx) || ctx <= 0) {
      setSubmitError('Context window must be a positive integer');
      return;
    }
    if (!Number.isFinite(mout) || mout <= 0) {
      setSubmitError('Max output tokens must be a positive integer');
      return;
    }
    if (!Number.isFinite(ic) || ic < 0) {
      setSubmitError('Input cost must be a non-negative number');
      return;
    }
    if (!Number.isFinite(oc) || oc < 0) {
      setSubmitError('Output cost must be a non-negative number');
      return;
    }

    const spec = {
      name: name.trim() || modelId,
      provider,
      contextWindow: ctx,
      maxOutputTokens: mout,
      inputCost: ic,
      outputCost: oc,
      cachedInputCost: Number.isFinite(cic) ? cic : 0,
      cachedOutputCost: Number.isFinite(coc) ? coc : 0,
    };

    setSaving(true);
    try {
      saveCustomModel(modelId, spec);
      if (isTauri()) {
        try {
          await invoke('set_model_capabilities', {
            modelId,
            supportsTemperature: !!sendsTemperature,
            supportsReasoningEffort: !!supportsReasoning,
          });
        } catch (e) {
          // Capability persistence failing shouldn't block the save — the
          // spec is still valid. Surface a soft warning instead.
          toast.error(`Model saved but capability flags failed: ${e}`);
        }
      }
      onSaved?.(spec);
      onOpenChange(false);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-h-[85vh] w-full max-w-md overflow-y-auto sm:max-w-md"
      >
        <DialogHeader>
          <DialogTitle>{isEdit ? 'Edit model' : 'Register model'}</DialogTitle>
          <DialogDescription>
            {isEdit
              ? `Update specs or capabilities for "${modelId}".`
              : `"${modelId}" isn't in the built-in model registry. Fill in its specs so cost and context-window calculations stay accurate.`}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3 pt-1">
          {templateOptions.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <Label className="text-xs">Use template (optional)</Label>
              <Select value={templateKey} onValueChange={applyTemplate}>
                <SelectTrigger size="sm">
                  <SelectValue placeholder="— start fresh —" />
                </SelectTrigger>
                <SelectContent>
                  {templateOptions.map((group) => (
                    <SelectGroup key={group.label}>
                      <SelectLabel>{group.label}</SelectLabel>
                      {group.items.map((it) => (
                        <SelectItem key={it.key} value={it.key}>
                          {it.label}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Display Name</Label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={modelId || ''}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Provider</Label>
            {providerType ? (
              <Input value={PROVIDER_LABELS[provider] || provider} disabled className="opacity-70" />
            ) : (
              <Select value={provider} onValueChange={setProvider}>
                <SelectTrigger size="sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PROVIDER_TYPES.map((p) => (
                    <SelectItem key={p} value={p}>
                      {PROVIDER_LABELS[p] || p}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Context Window (tokens)</Label>
            <NumberInput
              value={contextWindow}
              onChange={setContextWindow}
              placeholder="e.g. 200000"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Max Output Tokens</Label>
            <NumberInput
              value={maxOutput}
              onChange={setMaxOutput}
              placeholder="e.g. 64000"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Input Cost ($/1M tokens)</Label>
            <NumberInput
              value={inputCost}
              onChange={setInputCost}
              placeholder="$ per 1M tok"
              step="0.01"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Output Cost ($/1M tokens)</Label>
            <NumberInput
              value={outputCost}
              onChange={setOutputCost}
              placeholder="$ per 1M tok"
              step="0.01"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Cached Input Cost (optional)</Label>
            <NumberInput
              value={cachedInCost}
              onChange={setCachedInCost}
              placeholder="$ per 1M tok (optional)"
              step="0.01"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">Cached Output Cost (optional)</Label>
            <NumberInput
              value={cachedOutCost}
              onChange={setCachedOutCost}
              placeholder="$ per 1M tok (optional)"
              step="0.01"
            />
          </div>

          <div className="mt-1 flex flex-col gap-2">
            <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
              Capabilities
            </div>
            <label className="flex cursor-pointer items-center gap-2 text-xs">
              <Checkbox
                checked={sendsTemperature}
                onCheckedChange={(v) => setSendsTemperature(!!v)}
              />
              <span>
                Send temperature with requests
                <span className="ml-1 text-muted-foreground">
                  (uncheck if the model rejects it)
                </span>
              </span>
            </label>
            <label className="flex cursor-pointer items-center gap-2 text-xs">
              <Checkbox
                checked={supportsReasoning}
                onCheckedChange={(v) => setSupportsReasoning(!!v)}
              />
              <span>
                Supports reasoning / thinking effort
                <span className="ml-1 text-muted-foreground">
                  (uncheck for models that don't reason)
                </span>
              </span>
            </label>
          </div>

          {submitError && (
            <div className="text-xs text-destructive">{submitError}</div>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving ? 'Saving…' : isEdit ? 'Save changes' : 'Register'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default RegisterModelModal;
