import { el } from '../../utils/dom.js';
import { updateSetting, loadSettings } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';

export function createKeybindingsSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'Keybindings'));

  // Import button
  const importRow = el('div', { class: 'settings-row' });
  const importInfo = el('div', { class: 'settings-row__info' });
  importInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Import from VS Code'));
  importInfo.appendChild(el('div', { class: 'settings-row__desc' }, 'Import keybindings from a VS Code keybindings.json file'));
  importRow.appendChild(importInfo);

  const importBtn = el('button', { class: 'settings-btn' }, 'Import...');
  importBtn.addEventListener('click', async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const path = await open({
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (path) {
        await api.importKeybindings(path);
        await loadSettings();
      }
    } catch (e) {
      console.error('Failed to import keybindings:', e);
    }
  });
  importRow.appendChild(importBtn);
  container.appendChild(importRow);

  // Current keybindings list
  const bindings = settings.keybindings || [];
  if (bindings.length === 0) {
    container.appendChild(el('div', { class: 'settings-empty' }, 'No custom keybindings. Using defaults.'));
  } else {
    const table = el('div', { class: 'keybindings-table' });

    // Header
    const headerRow = el('div', { class: 'keybindings-row keybindings-row--header' });
    headerRow.appendChild(el('div', { class: 'keybindings-cell keybindings-cell--key' }, 'Key'));
    headerRow.appendChild(el('div', { class: 'keybindings-cell keybindings-cell--command' }, 'Command'));
    headerRow.appendChild(el('div', { class: 'keybindings-cell keybindings-cell--when' }, 'When'));
    table.appendChild(headerRow);

    for (const binding of bindings) {
      const row = el('div', { class: 'keybindings-row' });
      const keyCell = el('div', { class: 'keybindings-cell keybindings-cell--key' });
      keyCell.appendChild(el('kbd', { class: 'keybinding-key' }, binding.key));
      row.appendChild(keyCell);
      row.appendChild(el('div', { class: 'keybindings-cell keybindings-cell--command' }, binding.command));
      row.appendChild(el('div', { class: 'keybindings-cell keybindings-cell--when' }, binding.when || ''));
      table.appendChild(row);
    }

    container.appendChild(table);
  }

  return container;
}
