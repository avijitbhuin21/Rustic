import { el, icon, iconMulti } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { openSettings, closeSettings, settingsStore } from '../state/settings.js';
import { editorStore, SETTINGS_BUFFER_ID, setActiveBuffer } from '../state/editor.js';
import { gitStore, checkGitToken } from '../state/git.js';
import { createAccountPanel } from './account-panel.js';

// SVG path data for activity bar icons (Feather-style, viewBox 0 0 24 24)
const ICONS = {
  explorer: [
    'M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z',
    'M13 2v7h7',
  ],
  search: [
    'M11 17.25a6.25 6.25 0 1 1 0-12.5 6.25 6.25 0 0 1 0 12.5z',
    'M16 16l4.5 4.5',
  ],
  git: [
    'M6 3v12',
    'M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z',
    'M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6z',
    'M18 9a9 9 0 0 1-9 9',
  ],
  agent: [
    'M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.582a.5.5 0 0 1 0 .963L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z',
    'M20 3v4',
    'M22 5h-4',
  ],
  settings: [
    'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z',
    'M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z',
  ],
  account: [
    'M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2',
    'M12 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8z',
  ],
};

function createActivityItem(id, paths, title) {
  const btn = el('button', { class: 'activity-bar__item', title, dataset: { panel: id } });
  if (id === 'agent') {
    const img = el('img', {
      class: 'activity-bar__item-img',
      src: new URL('../rustic_agent.png', import.meta.url).href,
      alt: title,
      draggable: 'false',
    });
    btn.appendChild(img);
  } else {
    btn.appendChild(iconMulti(paths, 20));
  }

  btn.addEventListener('click', () => {
    const current = uiStore.getState('activePanel');
    if (current === id && uiStore.getState('primarySidebarVisible')) {
      uiStore.setState({ primarySidebarVisible: false });
    } else {
      uiStore.setState({ activePanel: id, primarySidebarVisible: true });
    }
  });

  return btn;
}

export function createActivityBar() {
  const bar = el('div', { class: 'activity-bar' });

  const gitBtn = createActivityItem('git', ICONS.git, 'Source Control');

  // Badge on the Source Control activity bar icon — shows total changes across all open projects
  const gitBadge = el('span', { class: 'activity-bar__badge' });
  gitBadge.style.display = 'none';
  gitBtn.appendChild(gitBadge);

  function updateGitBadge() {
    const statuses = gitStore.getState('projectStatuses') || {};
    let total = 0;
    for (const id in statuses) {
      const status = statuses[id];
      if (status && Array.isArray(status.files)) {
        total += status.files.length;
      }
    }
    if (total > 0) {
      gitBadge.textContent = total > 99 ? '99+' : String(total);
      gitBadge.style.display = '';
      gitBtn.title = `Source Control (${total} change${total === 1 ? '' : 's'})`;
    } else {
      gitBadge.textContent = '';
      gitBadge.style.display = 'none';
      gitBtn.title = 'Source Control';
    }
  }

  gitStore.subscribe('projectStatuses', updateGitBadge);
  updateGitBadge();

  const top = el('div', { class: 'activity-bar__top' }, [
    createActivityItem('agent', ICONS.agent, 'Agent'),
    createActivityItem('explorer', ICONS.explorer, 'Explorer'),
    createActivityItem('search', ICONS.search, 'Search'),
    gitBtn,
  ]);

  // Settings button with special behavior (opens full-page settings, not sidebar)
  const settingsBtn = el('button', { class: 'activity-bar__item', title: 'Settings', dataset: { panel: 'settings' } });
  settingsBtn.appendChild(iconMulti(ICONS.settings, 20));
  settingsBtn.addEventListener('click', () => {
    const activeId = editorStore.getState('activeBufferId');
    if (activeId === SETTINGS_BUFFER_ID) {
      // Already viewing settings — close it
      closeSettings();
    } else if (settingsStore.getState('isOpen')) {
      // Settings tab exists but not active — switch to it
      setActiveBuffer(SETTINGS_BUFFER_ID);
    } else {
      openSettings();
    }
  });

  // Account button
  const accountBtn = el('button', { class: 'activity-bar__item', title: 'Account', dataset: { panel: 'account' } });
  accountBtn.appendChild(iconMulti(ICONS.account, 20));
  accountBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    createAccountPanel(accountBtn);
  });

  // Show indicator when logged in
  checkGitToken();
  gitStore.subscribe('hasToken', (has) => {
    accountBtn.classList.toggle('activity-bar__item--authed', has);
  });

  const bottom = el('div', { class: 'activity-bar__bottom' }, [
    accountBtn,
    settingsBtn,
  ]);

  bar.appendChild(top);
  bar.appendChild(bottom);

  // Update active state
  function updateActive(activePanel) {
    bar.querySelectorAll('.activity-bar__item').forEach(item => {
      item.classList.toggle('active', item.dataset.panel === activePanel);
    });
  }

  uiStore.subscribe('activePanel', updateActive);
  updateActive(uiStore.getState('activePanel'));

  return bar;
}
