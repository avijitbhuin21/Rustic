import React, { useState } from 'react';
import { Settings2 } from 'lucide-react';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';
import { useSettings } from '@/state/settings';
import { SettingsSection, SettingRow } from './setting-row';
import { FormattersModal } from './formatters-modal';

export function EditorSettings() {
  const s = useSettings((s) => s.settings);
  const update = useSettings((u) => u.update);
  if (!s) return null;
  const e = s.editor ?? {};

  const set = (patch) => update({ editor: { ...e, ...patch } });
  const [formattersOpen, setFormattersOpen] = useState(false);

  // Open the Formatters modal automatically the first time the user turns
  // format-on-save ON — they almost certainly need to install or detect a
  // formatter for it to do anything. Calling set+modal in the same handler
  // keeps the toggle responsive (no extra round-trip).
  function handleFormatOnSave(v) {
    set({ format_on_save: v });
    if (v && e.format_on_save !== true) setFormattersOpen(true);
  }

  return (
    <>
      <SettingsSection title="Tab & Indentation">
        <SettingRow
          label="Tab Size"
          description="Number of spaces per tab"
          htmlFor="tab-size"
        >
          <Input
            id="tab-size"
            type="number"
            min={1}
            max={8}
            value={e.tab_size ?? 4}
            onChange={(ev) => set({ tab_size: Number(ev.target.value) })}
            className="h-7 w-20 text-xs"
          />
        </SettingRow>
        <SettingRow
          label="Insert Spaces"
          description="Use spaces instead of tab characters"
          htmlFor="insert-spaces"
        >
          <Switch
            id="insert-spaces"
            checked={e.insert_spaces !== false}
            onCheckedChange={(v) => set({ insert_spaces: v })}
          />
        </SettingRow>
        <SettingRow
          label="Auto Indent"
          description="Automatic indentation strategy when typing"
          htmlFor="auto-indent"
        >
          <Select value={e.auto_indent ?? 'advanced'} onValueChange={(v) => set({ auto_indent: v })}>
            <SelectTrigger id="auto-indent" className="h-7 w-32 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">None</SelectItem>
              <SelectItem value="keep">Keep</SelectItem>
              <SelectItem value="brackets">Brackets</SelectItem>
              <SelectItem value="advanced">Advanced</SelectItem>
              <SelectItem value="full">Full</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="Display">
        <SettingRow
          label="Word Wrap"
          description="Wrap long lines at the viewport edge"
          htmlFor="word-wrap"
        >
          <Switch
            id="word-wrap"
            checked={!!e.word_wrap}
            onCheckedChange={(v) => set({ word_wrap: v })}
          />
        </SettingRow>
        <SettingRow
          label="Line Numbers"
          description="Show line numbers in the gutter"
          htmlFor="line-numbers"
        >
          <Switch
            id="line-numbers"
            checked={e.line_numbers !== false}
            onCheckedChange={(v) => set({ line_numbers: v })}
          />
        </SettingRow>
        <SettingRow
          label="Minimap"
          description="Show a minimap overview of the file"
          htmlFor="minimap"
        >
          <Switch
            id="minimap"
            checked={!!e.minimap}
            onCheckedChange={(v) => set({ minimap: v })}
          />
        </SettingRow>
        <SettingRow
          label="Render Whitespace"
          description="Show whitespace characters"
          htmlFor="render-whitespace"
        >
          <Select
            value={e.render_whitespace ?? 'none'}
            onValueChange={(v) => set({ render_whitespace: v })}
          >
            <SelectTrigger id="render-whitespace" className="h-7 w-32 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">none</SelectItem>
              <SelectItem value="boundary">boundary</SelectItem>
              <SelectItem value="selection">selection</SelectItem>
              <SelectItem value="trailing">trailing</SelectItem>
              <SelectItem value="all">all</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
        <SettingRow
          label="Show Zero-Width Characters"
          description="Highlight invisible zero-width characters (U+200B, U+200C, U+200D, U+FEFF, etc.)"
          htmlFor="show-zwc"
        >
          <Switch
            id="show-zwc"
            checked={!!e.show_zero_width_characters}
            onCheckedChange={(v) => set({ show_zero_width_characters: v })}
          />
        </SettingRow>
        <SettingRow
          label="Bracket Pair Colorization"
          description="Colorize matching bracket pairs for easier code reading"
          htmlFor="bracket-pair"
        >
          <Switch
            id="bracket-pair"
            checked={e.bracket_pair_colorization !== false}
            onCheckedChange={(v) => set({ bracket_pair_colorization: v })}
          />
        </SettingRow>
        <SettingRow
          label="Format on Save"
          description="Automatically fix indentation and formatting when saving a file"
          htmlFor="format-on-save"
        >
          <div className="flex items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setFormattersOpen(true)}
              className="h-7 gap-1.5 px-2 text-[11px] text-muted-foreground hover:text-foreground"
            >
              <Settings2 className="size-3" />
              Configure
            </Button>
            <Switch
              id="format-on-save"
              checked={e.format_on_save !== false}
              onCheckedChange={handleFormatOnSave}
            />
          </div>
        </SettingRow>
        <SettingRow
          label="Sticky Scroll"
          description="Keep enclosing scopes pinned to the top of the viewport while scrolling"
          htmlFor="sticky-scroll"
        >
          <Switch
            id="sticky-scroll"
            checked={e.sticky_scroll !== false}
            onCheckedChange={(v) => set({ sticky_scroll: v })}
          />
        </SettingRow>
        <SettingRow
          label="Smooth Scrolling"
          description="Animate the editor when scrolling vertically"
          htmlFor="smooth-scrolling"
        >
          <Switch
            id="smooth-scrolling"
            checked={e.smooth_scrolling !== false}
            onCheckedChange={(v) => set({ smooth_scrolling: v })}
          />
        </SettingRow>
        <SettingRow
          label="Indent Guides"
          description="Render vertical guides at each indent level"
          htmlFor="indent-guides"
        >
          <Switch
            id="indent-guides"
            checked={e.indent_guides !== false}
            onCheckedChange={(v) => set({ indent_guides: v })}
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="Cursor">
        <SettingRow
          label="Cursor Blink"
          description="Animate the cursor"
          htmlFor="cursor-blink"
        >
          <Switch
            id="cursor-blink"
            checked={e.cursor_blink !== false}
            onCheckedChange={(v) => set({ cursor_blink: v })}
          />
        </SettingRow>
        <SettingRow
          label="Cursor Style"
          description="Shape of the text cursor"
          htmlFor="cursor-style"
        >
          <Select value={e.cursor_style ?? 'line'} onValueChange={(v) => set({ cursor_style: v })}>
            <SelectTrigger id="cursor-style" className="h-7 w-32 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="line">Line</SelectItem>
              <SelectItem value="block">Block</SelectItem>
              <SelectItem value="underline">Underline</SelectItem>
              <SelectItem value="line-thin">Line (thin)</SelectItem>
              <SelectItem value="block-outline">Block (outline)</SelectItem>
              <SelectItem value="underline-thin">Underline (thin)</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
        <SettingRow
          label="Smooth Caret Animation"
          description="Animate the cursor as it moves between positions"
          htmlFor="cursor-smooth"
        >
          <Select
            value={e.cursor_smooth_caret ?? 'off'}
            onValueChange={(v) => set({ cursor_smooth_caret: v })}
          >
            <SelectTrigger id="cursor-smooth" className="h-7 w-32 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="off">Off</SelectItem>
              <SelectItem value="explicit">Explicit</SelectItem>
              <SelectItem value="on">On</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
      </SettingsSection>

      <FormattersModal open={formattersOpen} onClose={() => setFormattersOpen(false)} />
    </>
  );
}
