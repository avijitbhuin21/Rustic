import { el, icon } from '../../utils/dom.js';
import { createCollapsible } from './settings-controls.js';
import { createAiSettings } from './ai-settings.js';
import { createMcpConfig, createMcpHeaderActions } from '../agent/mcp-config.js';
import { createSkillsPanel, createSkillsHeaderActions } from '../agent/skills-panel.js';
import { createWorkflowsPanel, createWorkflowsHeaderActions } from '../agent/workflows-panel.js';
import { createRulesPanel, createRulesHeaderActions } from '../agent/rules-panel.js';

export function createAgentSettings(settings) {
  const container = el('div', { class: 'settings-section' });

  // --- AI Providers ---
  const aiContent = el('div', { class: 'settings-collapsible-content' });
  const aiPanel = createAiSettings();
  aiContent.appendChild(aiPanel);

  const aiActions = el('div');
  const addCompatBtn = el('button', {
    class: 'settings-collapsible__action-btn',
    title: 'Add OpenAI-compatible provider',
  });
  addCompatBtn.appendChild(icon('M12 5v14M5 12h14', 14));
  addCompatBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    aiPanel.addCompatibleProvider?.();
  });
  aiActions.appendChild(addCompatBtn);

  container.appendChild(createCollapsible('AI Providers', aiContent, true, aiActions));

  // --- MCP Servers ---
  const mcpContent = el('div', { class: 'settings-collapsible-content' });
  const mcpPanel = createMcpConfig();
  mcpContent.appendChild(mcpPanel);
  const mcpActions = createMcpHeaderActions(
    () => mcpPanel._openEditJson?.(),
    () => mcpPanel._openAddNew?.(),
  );
  container.appendChild(createCollapsible('MCP Servers', mcpContent, false, mcpActions));

  // --- Skills (global) ---
  const skillsContent = el('div', { class: 'settings-collapsible-content' });
  const skillsPanel = createSkillsPanel();
  skillsContent.appendChild(skillsPanel);
  const skillsActions = createSkillsHeaderActions(
    () => skillsPanel._onPlus?.(),
    () => skillsPanel._onInfo?.(),
  );
  container.appendChild(createCollapsible('Skills', skillsContent, false, skillsActions));

  // --- Workflows (global) ---
  const workflowsContent = el('div', { class: 'settings-collapsible-content' });
  const workflowsPanel = createWorkflowsPanel();
  workflowsContent.appendChild(workflowsPanel);
  const workflowsActions = createWorkflowsHeaderActions(
    () => workflowsPanel._onPlus?.(),
    () => workflowsPanel._onInfo?.(),
  );
  container.appendChild(createCollapsible('Workflows', workflowsContent, false, workflowsActions));

  // --- Rules (global definitions, per-project activation) ---
  const rulesContent = el('div', { class: 'settings-collapsible-content' });
  const rulesPanel = createRulesPanel();
  rulesContent.appendChild(rulesPanel);
  const rulesActions = createRulesHeaderActions(
    () => rulesPanel._onPlus?.(),
    () => rulesPanel._onInfo?.(),
  );
  container.appendChild(createCollapsible('Rules', rulesContent, false, rulesActions));

  return container;
}
