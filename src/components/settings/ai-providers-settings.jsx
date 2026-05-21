import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { SettingsSection, SettingRow } from './setting-row';

// Maps the friendly slug used in the UI to the backend `ProviderType` variant
// name expected by `set_ai_provider`. Keep in sync with
// `src-tauri/src/commands/agent/mod.rs`.
const PROVIDERS = [
  { slug: 'anthropic', label: 'Anthropic (Claude)', providerType: 'Claude', defaultModel: 'claude-sonnet-4-5' },
  { slug: 'openai', label: 'OpenAI', providerType: 'OpenAi', defaultModel: 'gpt-5-mini' },
  { slug: 'gemini', label: 'Google Gemini', providerType: 'Gemini', defaultModel: 'gemini-2.5-flash' },
  { slug: 'openai-compatible', label: 'OpenAI-compatible', providerType: 'Compatible', defaultModel: '' },
];

export function AiProvidersSettings() {
  const [config, setConfig] = useState(null);
  const [newKeys, setNewKeys] = useState({});
  const [newModels, setNewModels] = useState({});

  useEffect(() => {
    invoke('get_ai_config').then(setConfig).catch(() => setConfig({ providers: {} }));
  }, []);

  const save = async (entry) => {
    const apiKey = (newKeys[entry.slug] || '').trim();
    const model = (newModels[entry.slug] || entry.defaultModel || '').trim();
    if (!apiKey || !model) return;
    await invoke('set_ai_provider', {
      providerType: entry.providerType,
      apiKey,
      model,
      baseUrl: null,
      name: null,
    });
    const next = await invoke('get_ai_config');
    setConfig(next);
    setNewKeys((p) => ({ ...p, [entry.slug]: '' }));
    setNewModels((p) => ({ ...p, [entry.slug]: '' }));
  };

  const remove = async (providerKey) => {
    await invoke('remove_ai_provider', { providerKey });
    const next = await invoke('get_ai_config');
    setConfig(next);
  };

  if (!config) return <div className="text-xs text-muted-foreground">Loading…</div>;

  return (
    <SettingsSection title="Providers">
      {PROVIDERS.map((p) => {
        // The agent config stores entries keyed by the backend `provider_key`.
        // Until we resolve the exact key shape here, look up by either the
        // backend variant or the UI slug — both have been observed in storage.
        const configured = config.providers?.[p.providerType] || config.providers?.[p.slug];
        const storedKey = configured?.provider_key || p.providerType;
        return (
          <SettingRow key={p.slug} label={p.label}>
            <div className="flex items-center gap-2">
              {configured ? (
                <>
                  <Badge variant="secondary" className="text-[10px]">Configured</Badge>
                  <Button variant="ghost" size="xs" onClick={() => remove(storedKey)}>Remove</Button>
                </>
              ) : (
                <div className="flex items-center gap-1">
                  <Input
                    type="password"
                    placeholder="API key"
                    value={newKeys[p.slug] ?? ''}
                    onChange={(e) => setNewKeys((prev) => ({ ...prev, [p.slug]: e.target.value }))}
                    className="h-7 w-40 text-xs"
                  />
                  <Input
                    type="text"
                    placeholder={p.defaultModel || 'model id'}
                    value={newModels[p.slug] ?? ''}
                    onChange={(e) => setNewModels((prev) => ({ ...prev, [p.slug]: e.target.value }))}
                    className="h-7 w-40 text-xs"
                  />
                  <Button
                    size="xs"
                    disabled={!(newKeys[p.slug] || '').trim()}
                    onClick={() => save(p)}
                  >
                    Save
                  </Button>
                </div>
              )}
            </div>
          </SettingRow>
        );
      })}
    </SettingsSection>
  );
}
