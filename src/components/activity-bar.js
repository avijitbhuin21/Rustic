import { el, icon, iconMulti } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { openSettings, closeSettings, settingsStore } from '../state/settings.js';
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
    'M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z',
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
  const svg = iconMulti(paths, 20);
  btn.appendChild(svg);

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

  const top = el('div', { class: 'activity-bar__top' }, [
    createActivityItem('explorer', ICONS.explorer, 'Explorer'),
    createActivityItem('search', ICONS.search, 'Search'),
    createActivityItem('git', ICONS.git, 'Source Control'),
    createActivityItem('agent', ICONS.agent, 'Agent'),
  ]);

  // Settings button with special behavior (opens full-page settings, not sidebar)
  const settingsBtn = el('button', { class: 'activity-bar__item', title: 'Settings', dataset: { panel: 'settings' } });
  settingsBtn.appendChild(iconMulti(ICONS.settings, 20));
  settingsBtn.addEventListener('click', () => {
    const isOpen = settingsStore.getState('isOpen');
    if (isOpen) {
      closeSettings();
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
