import { el } from '../../utils/dom.js';
import { createCollapsible } from './settings-controls.js';
import { createAiSettings } from './ai-settings.js';
import { createMcpConfig } from '../agent/mcp-config.js';

export function createAgentSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'Agent'));

  // --- AI Providers ---
  const aiContent = el('div', { class: 'settings-collapsible-content' });
  aiContent.appendChild(createAiSettings(settings));
  container.appendChild(createCollapsible('AI Providers', aiContent, true));

  // --- MCP Servers ---
  const mcpContent = el('div', { class: 'settings-collapsible-content' });
  mcpContent.appendChild(createMcpConfig());
  container.appendChild(createCollapsible('MCP Servers', mcpContent, false));

  // --- Skills ---
  const skillsContent = el('div', { class: 'settings-collapsible-content' });
  const skillsPlaceholder = el('div', { class: 'settings-coming-soon' });
  skillsPlaceholder.appendChild(el('div', { class: 'settings-coming-soon__icon' }, '⚡'));
  skillsPlaceholder.appendChild(el('div', { class: 'settings-coming-soon__title' }, 'Skills'));
  skillsPlaceholder.appendChild(el('div', { class: 'settings-coming-soon__text' }, 'Skills let the agent use specialized tools and capabilities. Configuration coming soon.'));
  skillsContent.appendChild(skillsPlaceholder);
  container.appendChild(createCollapsible('Skills', skillsContent, false));

  // --- Workflows ---
  const workflowsContent = el('div', { class: 'settings-collapsible-content' });
  const workflowsPlaceholder = el('div', { class: 'settings-coming-soon' });
  workflowsPlaceholder.appendChild(el('div', { class: 'settings-coming-soon__icon' }, '⚙'));
  workflowsPlaceholder.appendChild(el('div', { class: 'settings-coming-soon__title' }, 'Workflows'));
  workflowsPlaceholder.appendChild(el('div', { class: 'settings-coming-soon__text' }, 'Workflows allow you to automate multi-step agent tasks. Configuration coming soon.'));
  workflowsContent.appendChild(workflowsPlaceholder);
  container.appendChild(createCollapsible('Workflows', workflowsContent, false));

  return container;
}
