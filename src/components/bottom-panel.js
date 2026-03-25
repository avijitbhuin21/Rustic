import { el, icon } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { createTerminalTabs } from './terminal/terminal-tabs.js';
import { createTerminalPane } from './terminal/terminal-pane.js';

export function createBottomPanel() {
  const panel = el('div', { class: 'bottom-panel' });

  // Header
  const header = el('div', { class: 'bottom-panel__header' });

  const terminalTabs = createTerminalTabs();

  const actions = el('div', { class: 'bottom-panel__actions' });
  const minimizeBtn = el('button', { title: 'Minimize Panel' },
    icon('M18 6L6 18M6 6l12 12', 14)
  );
  minimizeBtn.addEventListener('click', () => {
    uiStore.setState({ bottomPanelVisible: false });
  });
  actions.appendChild(minimizeBtn);

  header.appendChild(terminalTabs);
  header.appendChild(actions);

  // Content
  const content = el('div', { class: 'bottom-panel__content' });
  content.appendChild(createTerminalPane());

  panel.appendChild(header);
  panel.appendChild(content);

  // React to visibility
  uiStore.subscribe('bottomPanelVisible', (visible) => {
    panel.style.display = visible ? 'flex' : 'none';
    document.documentElement.style.setProperty(
      '--panel-height',
      visible ? uiStore.getState('panelHeight') + 'px' : '0px'
    );
  });

  return panel;
}
