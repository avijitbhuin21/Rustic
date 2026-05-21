import React, { useEffect } from 'react';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useSettings } from '@/state/settings';
import { GeneralSettings } from './general-settings';
import { EditorSettings } from './editor-settings';
import { AppearanceSettings } from './appearance-settings';
import { AiProvidersSettings } from './ai-providers-settings';
import { KeybindingsSettings } from './keybindings-settings';

export function SettingsPanel() {
  const load = useSettings((s) => s.load);
  const loading = useSettings((s) => s.loading);
  const settings = useSettings((s) => s.settings);
  const error = useSettings((s) => s.error);

  useEffect(() => {
    if (!settings) load();
  }, [settings, load]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center border-b border-border/60 px-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          Settings
        </span>
      </div>
      {error && (
        <div className="px-3 py-2 text-xs text-destructive">Error: {error}</div>
      )}
      {loading && !settings && (
        <div className="px-3 py-4 text-xs text-muted-foreground">Loading settings…</div>
      )}
      {settings && (
        <Tabs defaultValue="general" className="flex flex-1 flex-col">
          <TabsList className="h-7 w-full justify-start gap-0 rounded-none border-b border-border/60 bg-transparent p-0">
            <SettingsTab value="general">General</SettingsTab>
            <SettingsTab value="editor">Editor</SettingsTab>
            <SettingsTab value="appearance">Appearance</SettingsTab>
            <SettingsTab value="keybindings">Keys</SettingsTab>
            <SettingsTab value="ai">AI</SettingsTab>
          </TabsList>
          <ScrollArea className="flex-1">
            <div className="p-3">
              <TabsContent value="general" className="mt-0"><GeneralSettings /></TabsContent>
              <TabsContent value="editor" className="mt-0"><EditorSettings /></TabsContent>
              <TabsContent value="appearance" className="mt-0"><AppearanceSettings /></TabsContent>
              <TabsContent value="keybindings" className="mt-0"><KeybindingsSettings /></TabsContent>
              <TabsContent value="ai" className="mt-0"><AiProvidersSettings /></TabsContent>
            </div>
          </ScrollArea>
        </Tabs>
      )}
    </div>
  );
}

function SettingsTab({ value, children }) {
  return (
    <TabsTrigger
      value={value}
      className="h-7 rounded-none border-b border-transparent px-2 text-[11px] data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:text-foreground"
    >
      {children}
    </TabsTrigger>
  );
}
