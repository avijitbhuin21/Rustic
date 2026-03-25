import { el, icon } from '../../utils/dom.js';
import { settingsStore, closeSettings, setCategory, updateSetting } from '../../state/settings.js';
import { createGeneralSettings } from './general-settings.js';
import { createEditorSettings } from './editor-settings.js';
import { createThemeSettings } from './theme-settings.js';
import { createAiSettings } from './ai-settings.js';
import { createKeybindingsSettings } from './keybindings-settings.js';

const categories = [
  { id: 'general', label: 'General', icon: 'M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z M15 12a3 3 0 11-6 0 3 3 0 016 0z' },
  { id: 'editor', label: 'Editor', icon: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z' },
  { id: 'appearance', label: 'Appearance', icon: 'M7 21a4 4 0 01-4-4V5a2 2 0 012-2h4a2 2 0 012 2v12a4 4 0 01-4 4zm0 0h12a2 2 0 002-2v-4a2 2 0 00-2-2h-2.343M11 7.343l1.657-1.657a2 2 0 012.828 0l2.829 2.829a2 2 0 010 2.828l-8.486 8.485M7 17h.01' },
  { id: 'keybindings', label: 'Keybindings', icon: 'M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707' },
  { id: 'ai', label: 'AI Providers', icon: 'M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z' },
];

export function createSettingsPanel() {
  const container = el('div', { class: 'settings-panel' });

  // Header
  const header = el('div', { class: 'settings-panel__header' });
  header.appendChild(el('h2', { class: 'settings-panel__title' }, 'Settings'));
  const closeBtn = el('button', { class: 'settings-panel__close', title: 'Close settings' });
  closeBtn.appendChild(icon('M6 18L18 6M6 6l12 12', 18));
  closeBtn.addEventListener('click', closeSettings);
  header.appendChild(closeBtn);
  container.appendChild(header);

  // Body
  const body = el('div', { class: 'settings-panel__body' });

  // Category sidebar
  const sidebar = el('div', { class: 'settings-panel__sidebar' });
  for (const cat of categories) {
    const item = el('div', { class: 'settings-category', 'data-category': cat.id });
    item.appendChild(icon(cat.icon, 16));
    item.appendChild(el('span', {}, cat.label));
    item.addEventListener('click', () => setCategory(cat.id));
    sidebar.appendChild(item);
  }
  body.appendChild(sidebar);

  // Content area
  const content = el('div', { class: 'settings-panel__content' });
  body.appendChild(content);

  container.appendChild(body);

  function render() {
    const activeCategory = settingsStore.getState('activeCategory');
    const settings = settingsStore.getState('settings');

    // Update active category highlight
    sidebar.querySelectorAll('.settings-category').forEach((item) => {
      item.classList.toggle('settings-category--active', item.dataset.category === activeCategory);
    });

    // Render content
    content.innerHTML = '';
    if (!settings) {
      content.appendChild(el('div', { class: 'settings-loading' }, 'Loading settings...'));
      return;
    }

    switch (activeCategory) {
      case 'general':
        content.appendChild(createGeneralSettings(settings));
        break;
      case 'editor':
        content.appendChild(createEditorSettings(settings));
        break;
      case 'appearance':
        content.appendChild(createThemeSettings(settings));
        break;
      case 'keybindings':
        content.appendChild(createKeybindingsSettings(settings));
        break;
      case 'ai':
        content.appendChild(createAiSettings(settings));
        break;
    }
  }

  settingsStore.subscribe('activeCategory', render);
  settingsStore.subscribe('settings', render);
  render();

  return container;
}
