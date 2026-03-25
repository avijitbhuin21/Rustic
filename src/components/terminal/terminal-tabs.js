import { el, icon } from '../../utils/dom.js';
import { terminalStore, closeTerminal, setActiveSession, createTerminal } from '../../state/terminal.js';

export function createTerminalTabs() {
  const container = el('div', { class: 'terminal-tabs' });

  function render() {
    container.innerHTML = '';

    const sessions = terminalStore.getState('sessions');
    const activeId = terminalStore.getState('activeSessionId');

    for (const session of sessions) {
      const isActive = session.id === activeId;

      const tab = el('button', {
        class: `terminal-tabs__tab ${isActive ? 'terminal-tabs__tab--active' : ''}`,
      });

      // Agent icon or terminal icon
      const tabIcon = session.is_agent
        ? icon('M12 2a2 2 0 0 1 2 2c0 .74-.4 1.39-1 1.73V7h1a7 7 0 0 1 7 7h1a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-1.07A7 7 0 0 1 14 22h-4a7 7 0 0 1-6.93-6H2a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1h1a7 7 0 0 1 7-7h1V5.73c-.6-.34-1-.99-1-1.73a2 2 0 0 1 2-2', 12)
        : icon('M4 17l6-6-6-6M12 19h8', 12);

      const label = el('span', { class: 'terminal-tabs__label' }, session.label);

      const closeBtn = el('span', { class: 'terminal-tabs__close' });
      closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 10));
      closeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        closeTerminal(session.id);
      });

      tab.appendChild(tabIcon);
      tab.appendChild(label);
      tab.appendChild(closeBtn);

      tab.addEventListener('click', () => setActiveSession(session.id));

      container.appendChild(tab);
    }

    // "+" button to create new terminal
    const addBtn = el('button', {
      class: 'terminal-tabs__add',
      title: 'New Terminal',
    });
    addBtn.appendChild(icon('M12 5v14M5 12h14', 14));
    addBtn.addEventListener('click', () => createTerminal());
    container.appendChild(addBtn);
  }

  terminalStore.subscribe('sessions', render);
  terminalStore.subscribe('activeSessionId', render);

  render();
  return container;
}
