import React from 'react';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { useSettings } from '@/state/settings';
import { SettingsSection, SettingRow } from './setting-row';

export function GeneralSettings() {
  const s = useSettings((s) => s.settings);
  const update = useSettings((u) => u.update);
  if (!s) return null;
  const g = s.general ?? {};

  return (
    <>
      <SettingsSection title="Startup">
        <SettingRow
          label="Restore last session"
          description="Reopen previously open files when launching."
          htmlFor="restore-session"
        >
          <Switch
            id="restore-session"
            checked={!!g.restore_last_session}
            onCheckedChange={(v) => update({ general: { ...g, restore_last_session: v } })}
          />
        </SettingRow>
        <SettingRow
          label="Confirm before quit"
          htmlFor="confirm-quit"
        >
          <Switch
            id="confirm-quit"
            checked={!!g.confirm_quit}
            onCheckedChange={(v) => update({ general: { ...g, confirm_quit: v } })}
          />
        </SettingRow>
      </SettingsSection>
      <SettingsSection title="Telemetry">
        <SettingRow
          label="Anonymous usage stats"
          description="Send anonymous error reports to help improve Rustic."
          htmlFor="telemetry"
        >
          <Switch
            id="telemetry"
            checked={!!g.telemetry}
            onCheckedChange={(v) => update({ general: { ...g, telemetry: v } })}
          />
        </SettingRow>
      </SettingsSection>
    </>
  );
}
