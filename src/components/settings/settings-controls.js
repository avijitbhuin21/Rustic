import { el, icon } from '../../utils/dom.js';

/**
 * Creates a collapsible section with a header that toggles content visibility.
 */
export function createCollapsible(title, contentEl, startOpen = true) {
  const wrapper = el('div', { class: 'settings-collapsible' + (startOpen ? ' settings-collapsible--open' : '') });

  const header = el('div', { class: 'settings-collapsible__header' });
  const chevron = el('span', { class: 'settings-collapsible__chevron' });
  chevron.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="9 18 15 12 9 6"/></svg>`;
  header.appendChild(chevron);
  header.appendChild(el('span', { class: 'settings-collapsible__title' }, title));

  const body = el('div', { class: 'settings-collapsible__body' });
  body.appendChild(contentEl);

  header.addEventListener('click', () => {
    wrapper.classList.toggle('settings-collapsible--open');
  });

  wrapper.appendChild(header);
  wrapper.appendChild(body);
  return wrapper;
}

export function createTextSetting(label, description, value, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const input = el('input', { class: 'settings-input', type: 'text', value: value || '' });
  input.addEventListener('change', () => onChange(input.value));
  row.appendChild(input);
  return row;
}

export function createNumberSetting(label, description, value, min, max, step, onChange) {
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
  input.addEventListener('input', () => onChange(parseFloat(input.value)));
  row.appendChild(input);
  return row;
}

export function createToggleSetting(label, description, value, onChange) {
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

export function createSelectSetting(label, description, value, options, onChange) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const select = el('select', { class: 'settings-select' });
  for (const opt of options) {
    const optEl = el('option', { value: opt.value ?? opt }, opt.label ?? opt);
    if ((opt.value ?? opt) === value) optEl.selected = true;
    select.appendChild(optEl);
  }
  select.addEventListener('change', () => onChange(select.value));
  row.appendChild(select);
  return row;
}

export function createTextareaSetting(label, description, value, placeholder, onChange) {
  const row = el('div', { class: 'settings-row settings-row--vertical' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const textarea = el('textarea', {
    class: 'settings-textarea',
    placeholder: placeholder || '',
    rows: '6',
  });
  textarea.value = value || '';
  textarea.addEventListener('change', () => onChange(textarea.value));
  row.appendChild(textarea);
  return row;
}

export function createButtonSetting(label, description, buttonText, onClick) {
  const row = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const btn = el('button', { class: 'settings-btn' }, buttonText);
  btn.addEventListener('click', onClick);
  row.appendChild(btn);
  return row;
}
