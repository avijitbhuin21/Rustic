import { el, icon } from '../../utils/dom.js';
import { settingsStore, updateSetting, loadSettings } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';
import { applyTheme } from '../../lib/theme.js';

export function createThemeSettings(settings) {
  const container = el('div', { class: 'settings-section' });

  // Theme selector
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, 'Color Theme'));
  info.appendChild(el('div', { class: 'settings-row__desc' }, 'Select a built-in or custom theme'));
  row.appendChild(info);

  const themes = settingsStore.getState('themes') || [];
  const select = el('select', { class: 'settings-select' });
  for (const theme of themes) {
    const opt = el('option', { value: theme.name },
      `${theme.name}${theme.is_builtin ? '' : ' (custom)'}${theme.kind === 'light' ? ' - Light' : ''}`
    );
    if (theme.name === settings.theme.active_theme) opt.selected = true;
    select.appendChild(opt);
  }
  select.addEventListener('change', async () => {
    await updateSetting('theme.active_theme', select.value);
    // Apply theme live
    try {
      const theme = await api.getActiveTheme();
      if (theme) applyTheme(theme);
    } catch (e) {
      console.error('Failed to apply theme:', e);
    }
  });
  row.appendChild(select);
  container.appendChild(row);

  // Import theme
  const importRow = el('div', { class: 'settings-row' });
  const importInfo = el('div', { class: 'settings-row__info' });
  importInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Import Theme'));
  importInfo.appendChild(el('div', { class: 'settings-row__desc' }, 'Import a theme from a TOML or JSON file'));
  importRow.appendChild(importInfo);

  const importBtn = el('button', { class: 'settings-btn' }, 'Import...');
  importBtn.addEventListener('click', async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const path = await open({
        filters: [{ name: 'Theme', extensions: ['toml', 'json'] }],
      });
      if (path) {
        const theme = await api.importTheme(path);
        if (theme) {
          await updateSetting('theme.active_theme', theme.name);
          applyTheme(theme);
          await loadSettings(); // refresh theme list
        }
      }
    } catch (e) {
      console.error('Failed to import theme:', e);
    }
  });
  importRow.appendChild(importBtn);
  container.appendChild(importRow);

  return container;
}
