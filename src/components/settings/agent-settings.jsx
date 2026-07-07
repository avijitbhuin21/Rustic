// Agent settings tabs — sections live in ./agent/* (split per A4).
import { IS_WEB } from '@/lib/platform';
import { AudioInputSection } from './agent/audio-section';
import { BudgetSection } from './agent/budget-section';
import { GithubAutoResolveSection } from './agent/github-section';
import { RulesSection, SkillsSection, WorkflowsSection } from './agent/library-sections';
import { McpSection } from './agent/mcp-section';
import { ProvidersSection } from './agent/providers-section';
import { AiConfigProvider, FlatSectionsContext } from './agent/shared';
import { SourceControlSection } from './agent/source-control-section';
import { SubAgentSection } from './agent/subagent-section';
import { ToolsSection } from './agent/tools-section';

// ─── Root ────────────────────────────────────────────────────────────────────

export function AgentTab({ children }) {
  return (
    <AiConfigProvider>
      <FlatSectionsContext.Provider value={true}>
        <div className="space-y-0">{children}</div>
      </FlatSectionsContext.Provider>
    </AiConfigProvider>
  );
}

export function AgentProvidersTab() {
  return (
    <AgentTab>
      <ProvidersSection />
    </AgentTab>
  );
}

export function AgentToolsTab() {
  return (
    <AgentTab>
      <ToolsSection />
      <McpSection />
    </AgentTab>
  );
}

export function AgentLibraryTab() {
  return (
    <AgentTab>
      <SkillsSection />
      <WorkflowsSection />
      <RulesSection />
    </AgentTab>
  );
}

export function AgentModelsTab() {
  return (
    <AgentTab>
      <SubAgentSection />
      <AudioInputSection />
      <SourceControlSection />
      <BudgetSection />
    </AgentTab>
  );
}

export function AgentGithubTab() {
  return (
    <AgentTab>
      <GithubAutoResolveSection />
    </AgentTab>
  );
}

export function AgentSettings() {
  return (
    <AiConfigProvider>
      <div className="space-y-0">
        <ProvidersSection />
        <SubAgentSection />
        <AudioInputSection />
        <SourceControlSection />
        <BudgetSection />
        {IS_WEB && <GithubAutoResolveSection />}
        <ToolsSection />
        <McpSection />
        <SkillsSection />
        <WorkflowsSection />
        <RulesSection />
      </div>
    </AiConfigProvider>
  );
}

export default AgentSettings;

