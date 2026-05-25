import React from 'react';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { useSettings } from '@/state/settings';
import { useEditor } from '@/state/editor';
import { SettingsSection, SettingRow } from './setting-row';

export function GeneralSettings() {
  const s = useSettings((s) => s.settings);
  const update = useSettings((u) => u.update);
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

      <SettingsSection title="Terminal">
        <SettingRow
          label="Open terminal in bottom panel"
          description="VS Code-style: new terminals open in a panel below the editor instead of as tabs. Existing terminals stay where they were opened."
          htmlFor="terminal-bottom"
        >
          <Switch
            id="terminal-bottom"
            checked={g.terminal_location === 'bottom'}
            onCheckedChange={(v) => {
              // Symmetric migration so existing terminals follow the new
              // setting instead of disappearing or getting orphaned:
              //   OFF → lift bottom-panel terminals into editor tabs.
              //   ON  → move terminal editor tabs down into the bottom panel.
              // The auto-visibility effect in App.jsx then shows/hides the
              // panel based on the resulting session locations.
              const editorActions = useEditor.getState();
              if (v) editorActions.migrateTabTerminalsToBottom();
              else editorActions.migrateBottomTerminalsToTabs();
              update({ general: { ...g, terminal_location: v ? 'bottom' : 'tab' } });
            }}
          />
        </SettingRow>
      </SettingsSection>
    </>
  );
}
