import { el } from '../../utils/dom.js';
import { updateSetting } from '../../state/settings.js';

export function createGeneralSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'General'));

  // Font family
  container.appendChild(createTextSetting(
    'Font Family',
    'Editor and terminal font family',
    settings.general.font_family,
    (v) => updateSetting('general.font_family', v)
  ));

  // Font size
  container.appendChild(createNumberSetting(
    'Font Size',
    'Editor font size in pixels',
    settings.general.font_size,
    8, 32, 1,
    (v) => updateSetting('general.font_size', v)
  ));

  // UI Scale
  container.appendChild(createNumberSetting(
    'UI Scale',
    'Scale the entire UI (1.0 = 100%)',
    settings.general.ui_scale,
    0.5, 2.0, 0.1,
    (v) => updateSetting('general.ui_scale', v)
  ));

  // Auto save
  container.appendChild(createToggleSetting(
    'Auto Save',
    'Automatically save files after a delay',
    settings.general.auto_save,
    (v) => updateSetting('general.auto_save', v)
  ));

  // Auto save delay
  container.appendChild(createNumberSetting(
    'Auto Save Delay',
    'Delay in milliseconds before auto-saving',
    settings.general.auto_save_delay_ms,
    200, 10000, 100,
    (v) => updateSetting('general.auto_save_delay_ms', v)
  ));

  return container;
}

function createTextSetting(label, description, value, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const input = el('input', { class: 'settings-input', type: 'text', value });
  input.addEventListener('change', () => onChange(input.value));
  row.appendChild(input);
  return row;
}

function createNumberSetting(label, description, value, min, max, step, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const input = el('input', {
    class: 'settings-input settings-input--number',
    type: 'number',
    value: String(value),
    min: String(min),
    max: String(max),
    step: String(step),
  });
  input.addEventListener('change', () => onChange(parseFloat(input.value)));
  row.appendChild(input);
  return row;
}

function createToggleSetting(label, description, value, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const toggle = el('label', { class: 'settings-toggle' });
  const checkbox = el('input', { type: 'checkbox' });
  checkbox.checked = value;
  checkbox.addEventListener('change', () => onChange(checkbox.checked));
  toggle.appendChild(checkbox);
  toggle.appendChild(el('span', { class: 'settings-toggle__slider' }));
  row.appendChild(toggle);
  return row;
}
