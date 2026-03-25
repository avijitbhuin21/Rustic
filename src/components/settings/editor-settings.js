import { el } from '../../utils/dom.js';
import { updateSetting } from '../../state/settings.js';

export function createEditorSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'Editor'));

  // Tab size
  container.appendChild(createNumberRow(
    'Tab Size', 'Number of spaces per tab', settings.editor.tab_size, 1, 8, 1,
    (v) => updateSetting('editor.tab_size', v)
  ));

  // Insert spaces
  container.appendChild(createToggleRow(
    'Insert Spaces', 'Use spaces instead of tab characters',
    settings.editor.insert_spaces,
    (v) => updateSetting('editor.insert_spaces', v)
  ));

  // Word wrap
  container.appendChild(createToggleRow(
    'Word Wrap', 'Wrap long lines at the viewport edge',
    settings.editor.word_wrap,
    (v) => updateSetting('editor.word_wrap', v)
  ));

  // Line numbers
  container.appendChild(createToggleRow(
    'Line Numbers', 'Show line numbers in the gutter',
    settings.editor.line_numbers,
    (v) => updateSetting('editor.line_numbers', v)
  ));

  // Cursor blink
  container.appendChild(createToggleRow(
    'Cursor Blink', 'Animate the cursor',
    settings.editor.cursor_blink,
    (v) => updateSetting('editor.cursor_blink', v)
  ));

  // Cursor style
  container.appendChild(createSelectRow(
    'Cursor Style', 'Shape of the text cursor',
    settings.editor.cursor_style,
    ['line', 'block', 'underline'],
    (v) => updateSetting('editor.cursor_style', v)
  ));

  // Render whitespace
  container.appendChild(createSelectRow(
    'Render Whitespace', 'Show whitespace characters',
    settings.editor.render_whitespace,
    ['none', 'boundary', 'all'],
    (v) => updateSetting('editor.render_whitespace', v)
  ));

  return container;
}

function createNumberRow(label, desc, value, min, max, step, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, desc));
  row.appendChild(info);
  const input = el('input', {
    class: 'settings-input settings-input--number',
    type: 'number', value: String(value),
    min: String(min), max: String(max), step: String(step),
  });
  input.addEventListener('change', () => onChange(parseInt(input.value, 10)));
  row.appendChild(input);
  return row;
}

function createToggleRow(label, desc, value, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, desc));
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

function createSelectRow(label, desc, value, options, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, desc));
  row.appendChild(info);
  const select = el('select', { class: 'settings-select' });
  for (const opt of options) {
    const option = el('option', { value: opt }, opt);
    if (opt === value) option.selected = true;
    select.appendChild(option);
  }
  select.addEventListener('change', () => onChange(select.value));
  row.appendChild(select);
  return row;
}
