import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { useSettings } from '@/state/settings';
import { useEditor } from '@/state/editor';
import { SettingsSection, SettingRow } from './setting-row';
import { TunnelSettings } from './tunnel-settings';
import { PowerSettings } from './power-settings';
import { RemoteBackendSettings } from './remote-backend-settings';
import { IS_WEB } from '@/lib/platform';

export function GeneralSettings() {
  const s = useSettings((s) => s.settings);
  const update = useSettings((u) => u.update);
  const [openWithRustic, setOpenWithRustic] = useState(false);
  const isWindows = !IS_WEB && navigator.userAgent.includes('Windows');

  useEffect(() => {
    if (!isWindows) return;
    invoke('get_open_with_rustic')
      .then((v) => setOpenWithRustic(!!v))
      .catch(() => {});
  }, [isWindows]);

  const toggleOpenWithRustic = async (v) => {
    /** Registers/unregisters the Explorer "Open with Rustic" context-menu entry. */
    try {
      await invoke('set_open_with_rustic', { enabled: v });
      setOpenWithRustic(v);
      toast.success(v ? 'Added "Open with Rustic" to the Explorer menu' : 'Removed from the Explorer menu');
    } catch (e) {
      toast.error(`Could not update Explorer integration: ${e}`);
    }
  };

  if (!s) return null;
  const g = s.general ?? {};

  return (
    <>
      <SettingsSection title="Auto Save & UI">
        <SettingRow
          label="Auto Save"
          description="Automatically save files after a delay."
          htmlFor="auto-save"
        >
          <Switch
            id="auto-save"
            checked={!!g.auto_save}
            onCheckedChange={(v) => update({ general: { ...g, auto_save: v } })}
          />
        </SettingRow>
        <SettingRow
          label="Auto Save Delay"
          description="Delay in milliseconds before auto-saving."
          htmlFor="auto-save-delay"
        >
          <Input
            id="auto-save-delay"
            type="number"
            min={100}
            max={10000}
            step={100}
            value={g.auto_save_delay ?? 1000}
            onChange={(ev) => update({ general: { ...g, auto_save_delay: Number(ev.target.value) } })}
            className="h-7 w-24 text-xs"
          />
        </SettingRow>
        <SettingRow
          label="UI Scale"
          description="Scale the entire UI (1.0 = 100%)."
          htmlFor="ui-scale"
        >
          <Input
            id="ui-scale"
            type="number"
            min={0.5}
            max={3}
            step={0.1}
            value={g.ui_scale ?? 1}
            onChange={(ev) => update({ general: { ...g, ui_scale: Number(ev.target.value) } })}
            className="h-7 w-24 text-xs"
          />
        </SettingRow>
      </SettingsSection>

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

      {isWindows && (
        <SettingsSection title="OS Integration">
          <SettingRow
            label='"Open with Rustic" in Explorer'
            description="Adds a right-click context-menu entry for files and folders (current user only)."
            htmlFor="open-with-rustic"
          >
            <Switch
              id="open-with-rustic"
              checked={openWithRustic}
              onCheckedChange={toggleOpenWithRustic}
            />
          </SettingRow>
        </SettingsSection>
      )}

      <SettingsSection title="About">
        <SettingRow
          label="What's New"
          description="See what changed in the latest release."
        >
          <Button
            variant="outline"
            size="sm"
            className="h-7 text-xs"
            onClick={() => window.dispatchEvent(new CustomEvent('rustic:open-patch-notes'))}
          >
            View patch notes
          </Button>
        </SettingRow>
      </SettingsSection>

      {!IS_WEB && <RemoteBackendSettings />}
      {IS_WEB && <PowerSettings />}
      {IS_WEB && <TunnelSettings />}

    </>
  );
}
