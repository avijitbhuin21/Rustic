import React, { useEffect } from 'react';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { MessageSquare, Server, Scroll, BookOpen, Workflow } from 'lucide-react';
import { useAgent } from '@/state/agent';
import { AddProjectButton } from '@/components/shell/add-project-button';
import { ChatView } from './chat-view';
import { McpPanel } from './mcp-panel';
import { RulesPanel } from './rules-panel';
import { SkillsPanel } from './skills-panel';
import { WorkflowsPanel } from './workflows-panel';
import { PermissionPrompt } from './permission-prompt';
import { QuestionPrompt } from './question-prompt';

export default function AgentPanel() {
  const loadInitial = useAgent((s) => s.loadInitial);
  const bindListeners = useAgent((s) => s.bindListeners);

  useEffect(() => {
    loadInitial();
    let cleanup;
    bindListeners().then((fn) => {
      cleanup = fn;
    });
    return () => {
      if (typeof cleanup === 'function') cleanup();
    };
  }, [loadInitial, bindListeners]);

  return (
    <div className="flex h-full flex-col bg-sidebar">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          Agent
        </span>
        <AddProjectButton />
      </div>
      <Tabs defaultValue="chat" className="flex min-h-0 flex-1 flex-col gap-0">
        <TabsList className="mx-2 h-7 shrink-0 self-start" variant="line">
          <TabsTrigger value="chat" className="gap-1 text-xs">
            <MessageSquare className="size-3" /> Chat
          </TabsTrigger>
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
        <TabsContent value="chat" className="min-h-0 flex-1">
          <ChatView />
        </TabsContent>
        <TabsContent value="mcp" className="min-h-0 flex-1">
          <McpPanel />
        </TabsContent>
        <TabsContent value="rules" className="min-h-0 flex-1">
          <RulesPanel />
        </TabsContent>
        <TabsContent value="skills" className="min-h-0 flex-1">
          <SkillsPanel />
        </TabsContent>
        <TabsContent value="workflows" className="min-h-0 flex-1">
          <WorkflowsPanel />
        </TabsContent>
      </Tabs>
      <PermissionPrompt />
      <QuestionPrompt />
    </div>
  );
}
