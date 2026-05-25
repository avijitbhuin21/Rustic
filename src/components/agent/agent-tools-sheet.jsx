import React from 'react';
import { Server, Scroll, BookOpen, Workflow } from 'lucide-react';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetBody,
} from '@/components/ui/sheet';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { McpPanel } from './mcp-panel';
import { RulesPanel } from './rules-panel';
import { SkillsPanel } from './skills-panel';
import { WorkflowsPanel } from './workflows-panel';

// Hosts the previously-separate tabs (MCP / Rules / Skills / Workflows) inside
// a slide-over so the chat stays visible underneath. Driven from the kebab in
// ChatView's header. The `initialTab` prop lets the kebab menu jump straight
// to a specific section.
export function AgentToolsSheet({ open, onOpenChange, initialTab = 'mcp' }) {
  // Remount the inner Tabs whenever the sheet opens so `defaultValue` actually
  // picks up the requested initialTab — Radix Tabs treats defaultValue as
  // uncontrolled, so changing it later after mount is a no-op.
  const tabsKey = open ? initialTab : 'closed';

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="w-[480px]">
        <SheetHeader>
          <SheetTitle>Agent tools</SheetTitle>
        </SheetHeader>
        <SheetBody>
          <Tabs
            key={tabsKey}
            defaultValue={initialTab}
            className="flex h-full min-h-0 flex-col gap-0"
          >
            <TabsList className="mx-3 my-2 h-7 shrink-0 self-start" variant="line">
              <TabsTrigger value="mcp" className="gap-1 text-xs">
                <Server className="size-3" /> MCP
              </TabsTrigger>
              <TabsTrigger value="rules" className="gap-1 text-xs">
                <Scroll className="size-3" /> Rules
              </TabsTrigger>
              <TabsTrigger value="skills" className="gap-1 text-xs">
                <BookOpen className="size-3" /> Skills
              </TabsTrigger>
              <TabsTrigger value="workflows" className="gap-1 text-xs">
                <Workflow className="size-3" /> Workflows
              </TabsTrigger>
            </TabsList>
            <TabsContent value="mcp" className="min-h-0 flex-1 overflow-hidden">
              <McpPanel />
            </TabsContent>
            <TabsContent value="rules" className="min-h-0 flex-1 overflow-hidden">
              <RulesPanel />
            </TabsContent>
            <TabsContent value="skills" className="min-h-0 flex-1 overflow-hidden">
              <SkillsPanel />
            </TabsContent>
            <TabsContent value="workflows" className="min-h-0 flex-1 overflow-hidden">
              <WorkflowsPanel />
            </TabsContent>
          </Tabs>
        </SheetBody>
      </SheetContent>
    </Sheet>
  );
}

export default AgentToolsSheet;
