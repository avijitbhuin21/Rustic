import { el } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { createExplorer } from './explorer/explorer.js';
import { createSearchPanel } from './search/search-panel.js';
import { createSourceControl } from './git/source-control.js';
import { createAgentPanel } from './agent/agent-panel.js';

const PANEL_TITLES = {
  explorer: 'Explorer',
  search: 'Search',
  git: 'Source Control',
  agent: 'Agent',
  settings: 'Settings',
};

// Panel components
const panelCreators = {
  explorer: createExplorer,
  search: createSearchPanel,
  git: createSourceControl,
  agent: createAgentPanel,
  settings: () => el('div', { class: 'panel-placeholder' }, 'Settings panel'),
};

export function createPrimarySidebar() {
  const sidebar = el('aside', { class: 'primary-sidebar', 'aria-label': 'Primary sidebar' });
  let currentPanel = null;

  function renderPanel(panelId) {
    if (currentPanel === panelId) return;
    currentPanel = panelId;

    // Remove all children except resize handles (added by main.js)
    const children = Array.from(sidebar.children);
    for (const child of children) {
      if (!child.classList.contains('resize-handle')) {
        sidebar.removeChild(child);
      }
    }
    const creator = panelCreators[panelId] || panelCreators.explorer;
    sidebar.appendChild(creator());
  }

  // React to panel changes
  uiStore.subscribe('activePanel', renderPanel);

  // React to visibility
  uiStore.subscribe('primarySidebarVisible', (visible) => {
    sidebar.style.display = visible ? 'flex' : 'none';
  });

  // Initial render
  renderPanel(uiStore.getState('activePanel'));

  return sidebar;
}
