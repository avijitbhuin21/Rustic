import React from 'react';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';
import { useSettings } from '@/state/settings';
import { SettingsSection, SettingRow } from './setting-row';

export function EditorSettings() {
  const s = useSettings((s) => s.settings);
  const update = useSettings((u) => u.update);
  if (!s) return null;
  const e = s.editor ?? {};

  const set = (patch) => update({ editor: { ...e, ...patch } });

  return (
    <>
      <SettingsSection title="Font">
        <SettingRow label="Font family" htmlFor="font-family">
          <Input
            id="font-family"
            value={e.font_family ?? ''}
            onChange={(ev) => set({ font_family: ev.target.value })}
            className="h-7 w-40 text-xs"
            placeholder="Consolas, monospace"
          />
        </SettingRow>
        <SettingRow label="Font size" htmlFor="font-size">
          <Input
            id="font-size"
            type="number"
            min={8}
            max={32}
            value={e.font_size ?? 13}
            onChange={(ev) => set({ font_size: Number(ev.target.value) })}
            className="h-7 w-20 text-xs"
          />
        </SettingRow>
        <SettingRow label="Line height" htmlFor="line-height">
          <Input
            id="line-height"
            type="number"
            min={1}
            max={3}
            step={0.1}
            value={e.line_height ?? 1.5}
            onChange={(ev) => set({ line_height: Number(ev.target.value) })}
            className="h-7 w-20 text-xs"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="Indentation">
        <SettingRow label="Tab size" htmlFor="tab-size">
          <Input
            id="tab-size"
            type="number"
            min={1}
            max={8}
            value={e.tab_size ?? 2}
            onChange={(ev) => set({ tab_size: Number(ev.target.value) })}
            className="h-7 w-20 text-xs"
          />
        </SettingRow>
        <SettingRow label="Insert spaces" htmlFor="insert-spaces">
          <Switch
            id="insert-spaces"
            checked={!!e.insert_spaces}
            onCheckedChange={(v) => set({ insert_spaces: v })}
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="Display">
        <SettingRow label="Word wrap" htmlFor="word-wrap">
          <Select value={e.word_wrap ?? 'off'} onValueChange={(v) => set({ word_wrap: v })}>
            <SelectTrigger id="word-wrap" className="h-7 w-32 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="off">Off</SelectItem>
              <SelectItem value="on">On</SelectItem>
              <SelectItem value="bounded">Bounded</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
        <SettingRow label="Line numbers" htmlFor="line-numbers">
          <Switch
            id="line-numbers"
            checked={e.line_numbers !== false}
            onCheckedChange={(v) => set({ line_numbers: v })}
          />
        </SettingRow>
        <SettingRow label="Minimap" htmlFor="minimap">
          <Switch
            id="minimap"
            checked={!!e.minimap}
            onCheckedChange={(v) => set({ minimap: v })}
          />
        </SettingRow>
        <SettingRow label="Sticky scroll" htmlFor="sticky-scroll">
          <Switch
            id="sticky-scroll"
            checked={!!e.sticky_scroll}
            onCheckedChange={(v) => set({ sticky_scroll: v })}
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="Language Servers (LSP)">
        <SettingRow
          label="Enable external LSPs"
          description="Off by default. Each LSP server can use 100-700 MB RAM. Enable per language below."
          htmlFor="lsp-enabled"
        >
          <Switch
            id="lsp-enabled"
            checked={!!e.lsp_enabled}
            onCheckedChange={(v) => set({ lsp_enabled: v })}
          />
        </SettingRow>
      </SettingsSection>
    </>
  );
}
