import { el } from '../../utils/dom.js';
import { createCollapsible } from './settings-controls.js';
import { createAiSettings } from './ai-settings.js';
import { createMcpConfig } from '../agent/mcp-config.js';
import { createSkillsPanel } from '../agent/skills-panel.js';
import { createWorkflowsPanel } from '../agent/workflows-panel.js';
import { workspaceStore } from '../../state/workspace.js';

export function createAgentSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'Agent'));

  // --- AI Providers ---
  const aiContent = el('div', { class: 'settings-collapsible-content' });
  aiContent.appendChild(createAiSettings());
  container.appendChild(createCollapsible('AI Providers', aiContent, true));

  // --- MCP Servers ---
  const mcpContent = el('div', { class: 'settings-collapsible-content' });
  mcpContent.appendChild(createMcpConfig());
  container.appendChild(createCollapsible('MCP Servers', mcpContent, false));

  // --- Skills ---
  const skillsContent = el('div', { class: 'settings-collapsible-content' });
  // Use the first active project id (if any) so the panel can list/manage project skills
  const activeProjectId = (() => {
    const projects = workspaceStore.getState('projects');
    return projects && projects.length > 0 ? projects[0].id : null;
  })();
  skillsContent.appendChild(createSkillsPanel(activeProjectId));
  container.appendChild(createCollapsible('Skills', skillsContent, false));

  // --- Workflows ---
  const workflowsContent = el('div', { class: 'settings-collapsible-content' });
  workflowsContent.appendChild(createWorkflowsPanel(activeProjectId));
  container.appendChild(createCollapsible('Workflows', workflowsContent, false));

  return container;
}
