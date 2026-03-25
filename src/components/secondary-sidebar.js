import { el } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { createChatView } from './agent/chat-view.js';

export function createSecondarySidebar() {
  const sidebar = el('div', { class: 'secondary-sidebar' });

  const header = el('div', { class: 'sidebar-header' });
  header.appendChild(el('span', {}, 'Agent Chat'));
  sidebar.appendChild(header);

  const content = el('div', { class: 'sidebar-content' });
  content.appendChild(createChatView());
  sidebar.appendChild(content);

  // React to visibility
  uiStore.subscribe('secondarySidebarVisible', (visible) => {
    sidebar.style.display = visible ? 'flex' : 'none';
    document.documentElement.style.setProperty(
      '--secondary-width',
      visible ? uiStore.getState('secondarySidebarWidth') + 'px' : '0px'
    );
  });

  // Initialize visibility
  sidebar.style.display = uiStore.getState('secondarySidebarVisible') ? 'flex' : 'none';

  return sidebar;
}
