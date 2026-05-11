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
  // Cached DOM node per panel id. Each panel is created once on first access
  // and then reused — switching tabs just toggles display. This keeps the
  // panel's internal state alive (search query + results, agent chat scroll,
  // explorer expanded folders, git diff selection, etc.) and avoids leaking
  // store subscriptions every time the user clicks away and back.
  const panelInstances = {};

  function renderPanel(panelId) {
    if (currentPanel === panelId) return;
    currentPanel = panelId;

    // Hide every cached panel; the active one is shown below.
    for (const id in panelInstances) {
      panelInstances[id].style.display = 'none';
    }

    // Lazy-create on first access so panels that the user never opens don't
    // pay any construction cost.
    if (!panelInstances[panelId]) {
      const creator = panelCreators[panelId] || panelCreators.explorer;
      const node = creator();
      panelInstances[panelId] = node;
      // Insert before the resize handle (added by main.js after this
      // function returns) so the handle stays at the visual edge.
      const handle = sidebar.querySelector('.resize-handle');
      if (handle) {
        sidebar.insertBefore(node, handle);
      } else {
        sidebar.appendChild(node);
      }
    }

    panelInstances[panelId].style.display = '';
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
