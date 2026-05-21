import React from 'react';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';
import { useSettings } from '@/state/settings';
import { SettingsSection, SettingRow } from './setting-row';

export function AppearanceSettings() {
  const themes = useSettings((s) => s.themes);
  const active = useSettings((s) => s.activeTheme);
  const update = useSettings((u) => u.update);
  const s = useSettings((s) => s.settings);
  if (!s) return null;
  const a = s.appearance ?? {};

  const set = (patch) => update({ appearance: { ...a, ...patch } });

  return (
    <>
      <SettingsSection title="Theme">
        <SettingRow label="Active theme" htmlFor="theme-select">
          <Select value={a.theme ?? active?.name ?? ''} onValueChange={(v) => set({ theme: v })}>
            <SelectTrigger id="theme-select" className="h-7 w-48 text-xs">
              <SelectValue placeholder="Select theme" />
            </SelectTrigger>
            <SelectContent>
              {themes.map((t) => (
                <SelectItem key={t.name} value={t.name}>{t.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingRow>
      </SettingsSection>
    </>
  );
}
