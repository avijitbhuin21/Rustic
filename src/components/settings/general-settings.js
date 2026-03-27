import { el } from '../../utils/dom.js';
import { updateSetting, loadSettings } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';
import { createCollapsible, createNumberSetting, createToggleSetting, createTextSetting } from './settings-controls.js';
import { createKeybindingsSection } from './keybindings-settings.js';

export function createGeneralSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'General'));

  // --- Auto Save section (collapsible) ---
  const autoSaveContent = el('div', { class: 'settings-collapsible-content' });

  autoSaveContent.appendChild(createToggleSetting(
    'Auto Save',
    'Automatically save files after a delay',
    settings.general.auto_save,
    (v) => updateSetting('general.auto_save', v)
  ));

  autoSaveContent.appendChild(createNumberSetting(
    'Auto Save Delay',
    'Delay in milliseconds before auto-saving',
    settings.general.auto_save_delay_ms,
    200, 10000, 100,
    (v) => updateSetting('general.auto_save_delay_ms', v)
  ));

  autoSaveContent.appendChild(createNumberSetting(
    'UI Scale',
    'Scale the entire UI (1.0 = 100%)',
    settings.general.ui_scale,
    0.5, 2.0, 0.1,
    (v) => updateSetting('general.ui_scale', v)
  ));

  container.appendChild(createCollapsible('Auto Save & UI', autoSaveContent, true));

  // --- Keybindings section (collapsible) ---
  const keybindingsContent = createKeybindingsSection(settings);
  container.appendChild(createCollapsible('Keybindings', keybindingsContent, false));

  // --- AI Providers section (collapsible, coming soon) ---
  const aiContent = el('div', { class: 'settings-collapsible-content' });
  const comingSoon = el('div', { class: 'settings-coming-soon' });
  comingSoon.appendChild(el('div', { class: 'settings-coming-soon__icon' }, '🤖'));
  comingSoon.appendChild(el('div', { class: 'settings-coming-soon__title' }, 'AI Providers'));
  comingSoon.appendChild(el('div', { class: 'settings-coming-soon__text' }, 'AI provider configuration is coming soon. You\'ll be able to connect Claude, OpenAI, Gemini, and OpenAI-compatible providers directly from here.'));
  aiContent.appendChild(comingSoon);

  container.appendChild(createCollapsible('AI Providers', aiContent, false));

  return container;
}
