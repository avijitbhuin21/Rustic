import { el, icon } from '../../utils/dom.js';
import { settingsStore, setCategory } from '../../state/settings.js';
import { createGeneralSettings } from './general-settings.js';
import { createEditorSettings } from './editor-settings.js';
import { createAppearanceSettings } from './appearance-settings.js';
import { createAgentSettings } from './agent-settings.js';
import { createShortcutsSettings } from './shortcuts-settings.js';
import { createLspSettings } from './lsp-settings.js';

const categories = [
  { id: 'general', label: 'General', icon: 'M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z M15 12a3 3 0 11-6 0 3 3 0 016 0z' },
  { id: 'appearance', label: 'Appearance', icon: 'M7 21a4 4 0 01-4-4V5a2 2 0 012-2h4a2 2 0 012 2v12a4 4 0 01-4 4zm0 0h12a2 2 0 002-2v-4a2 2 0 00-2-2h-2.343M11 7.343l1.657-1.657a2 2 0 012.828 0l2.829 2.829a2 2 0 010 2.828l-8.486 8.485M7 17h.01' },
  { id: 'editor', label: 'Editor', icon: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z' },
  { id: 'lsp', label: 'LSP', icon: 'M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z' },
  { id: 'shortcuts', label: 'Shortcuts', icon: 'M9 11l3 3L22 4M21 12v7a2 2 0 01-2 2H5a2 2 0 01-2-2V5a2 2 0 012-2h11' },
  { id: 'agent', label: 'Agent', icon: 'M9 3H5a2 2 0 00-2 2v4m6-6h10a2 2 0 012 2v4M9 3v18m0 0h10a2 2 0 002-2V9M9 21H5a2 2 0 01-2-2V9m0 0h18' },
];

export function createSettingsPanel() {
  const container = el('div', { class: 'settings-panel' });

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

    // Preserve collapsible open/closed states before re-render
    const collapsibleStates = {};
    content.querySelectorAll('.settings-collapsible').forEach((c) => {
      const title = c.querySelector('.settings-collapsible__title')?.textContent;
      if (title) collapsibleStates[title] = c.classList.contains('settings-collapsible--open');
    });

    // Preserve focused input so we can restore focus after re-render
    let focusedLabel = null;
    let focusedTag = null;
    const activeEl = content.contains(document.activeElement) ? document.activeElement : null;
    if (activeEl && (activeEl.tagName === 'INPUT' || activeEl.tagName === 'SELECT' || activeEl.tagName === 'TEXTAREA')) {
      focusedTag = activeEl.tagName;
      const row = activeEl.closest('.settings-row');
      if (row) {
        focusedLabel = row.querySelector('.settings-row__label')?.textContent;
      }
    }

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
        content.appendChild(createAppearanceSettings(settings));
        break;
      case 'lsp':
        content.appendChild(createLspSettings(settings));
        break;
      case 'shortcuts':
        content.appendChild(createShortcutsSettings(settings));
        break;
      case 'agent':
        content.appendChild(createAgentSettings(settings));
        break;
    }

    // Restore collapsible open/closed states after re-render
    if (Object.keys(collapsibleStates).length > 0) {
      content.querySelectorAll('.settings-collapsible').forEach((c) => {
        const title = c.querySelector('.settings-collapsible__title')?.textContent;
        if (title && title in collapsibleStates) {
          c.classList.toggle('settings-collapsible--open', collapsibleStates[title]);
        }
      });
    }

    // Restore focus to the matching input after re-render
    if (focusedLabel) {
      for (const row of content.querySelectorAll('.settings-row')) {
        if (row.querySelector('.settings-row__label')?.textContent === focusedLabel) {
          const target = row.querySelector(focusedTag || 'input');
          if (target) { target.focus(); break; }
        }
      }
    }
  }

  settingsStore.subscribe('activeCategory', render);
  settingsStore.subscribe('settings', render);
  render();

  return container;
}
