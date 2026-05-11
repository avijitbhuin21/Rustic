import { el } from './dom.js';

// ── Searchable combobox ──────────────────────────────────────────────────────
// A native <select> can't filter as you type; for the media-tool model picker
// the live list is often hundreds of entries (OpenRouter alone returns ~250),
// so this helper builds a custom input + popup that filters on each keystroke.
//
// `getOptions()` returns the current option array `[{ value, label, hint?, disabled? }]`
// — passed as a function so the dropdown picks up live updates (e.g. when the
// async model-list fetch resolves) without rebuilding the widget.
//
// `allowCustom: true` means free-typed values that don't match any option are
// accepted on Enter / blur and emitted via `onChange` as the raw string.
export function createCombobox({
  initialValue = '',
  placeholder = '',
  getOptions,
  onChange,
  allowCustom = false,
}) {
  const root = el('div', { class: 'combobox' });
  const input = el('input', { class: 'combobox__input', type: 'text', placeholder });
  const arrow = el('button', { class: 'combobox__arrow', type: 'button', tabindex: '-1' }, '▾');
  const panel = el('div', { class: 'combobox__panel combobox__panel--hidden' });

  let isOpen = false;
  let filter = '';
  let highlighted = 0;
  let currentValue = initialValue || '';

  function labelFor(value) {
    const opts = getOptions() || [];
    const found = opts.find((o) => o.value === value);
    return found ? found.label : value;
  }

  function getFiltered() {
    const opts = getOptions() || [];
    const f = filter.trim().toLowerCase();
    if (!f) return opts.filter((o) => !o.disabled || o.hint);
    return opts.filter((o) => {
      if (o.disabled && !o.hint) return false;
      return (
        (o.label || '').toLowerCase().includes(f) ||
        (o.value || '').toLowerCase().includes(f)
      );
    });
  }

  function rebuildList() {
    panel.replaceChildren();
    const list = getFiltered();
    if (highlighted >= list.length) highlighted = Math.max(0, list.length - 1);
    if (list.length === 0) {
      panel.appendChild(el('div', { class: 'combobox__empty' },
        allowCustom && filter.trim()
          ? 'No match — press Enter to use as custom value'
          : 'No matches'));
      return;
    }
    list.forEach((opt, idx) => {
      const item = el('div', { class: 'combobox__option' });
      if (opt.disabled) item.classList.add('combobox__option--disabled');
      if (idx === highlighted) item.classList.add('combobox__option--highlighted');
      if (opt.value === currentValue) item.classList.add('combobox__option--selected');
      item.appendChild(el('span', { class: 'combobox__option-label' }, opt.label));
      if (opt.hint) item.appendChild(el('span', { class: 'combobox__option-hint' }, opt.hint));
      // mousedown rather than click so it fires before the input's blur handler.
      item.addEventListener('mousedown', (ev) => {
        ev.preventDefault();
        if (opt.disabled) return;
        commit(opt.value);
      });
      panel.appendChild(item);
    });
  }

  function open() {
    if (isOpen) return;
    isOpen = true;
    filter = '';
    input.value = '';
    highlighted = 0;
    panel.classList.remove('combobox__panel--hidden');
    rebuildList();
  }

  function close({ restore = true } = {}) {
    if (!isOpen) return;
    isOpen = false;
    panel.classList.add('combobox__panel--hidden');
    if (restore) input.value = labelFor(currentValue);
  }

  function commit(value) {
    currentValue = value;
    input.value = labelFor(value);
    close({ restore: false });
    if (onChange) onChange(value);
  }

  input.value = labelFor(currentValue);

  input.addEventListener('focus', open);
  input.addEventListener('click', open);
  input.addEventListener('input', () => {
    if (!isOpen) open();
    filter = input.value;
    highlighted = 0;
    rebuildList();
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const list = getFiltered();
      if (list.length > 0 && highlighted >= 0 && highlighted < list.length) {
        commit(list[highlighted].value);
      } else if (allowCustom && filter.trim()) {
        commit(filter.trim());
      } else {
        close();
      }
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!isOpen) { open(); return; }
      const list = getFiltered();
      highlighted = Math.min(highlighted + 1, list.length - 1);
      rebuildList();
      scrollHighlightedIntoView();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (!isOpen) { open(); return; }
      highlighted = Math.max(highlighted - 1, 0);
      rebuildList();
      scrollHighlightedIntoView();
    }
  });
  input.addEventListener('blur', () => {
    // Defer so an option mousedown can land first.
    setTimeout(() => {
      if (root.contains(document.activeElement)) return;
      if (!isOpen) return;
      // If the user typed something and there's an exact match, accept it.
      const f = filter.trim();
      if (f) {
        const exact = (getOptions() || []).find((o) => o.value === f || o.label === f);
        if (exact && !exact.disabled) {
          commit(exact.value);
          return;
        }
        if (allowCustom) {
          commit(f);
          return;
        }
      }
      close();
    }, 0);
  });
  arrow.addEventListener('mousedown', (e) => {
    e.preventDefault();
    if (isOpen) close();
    else input.focus();
  });

  function scrollHighlightedIntoView() {
    const hi = panel.querySelector('.combobox__option--highlighted');
    if (hi && hi.scrollIntoView) hi.scrollIntoView({ block: 'nearest' });
  }

  root.appendChild(input);
  root.appendChild(arrow);
  root.appendChild(panel);

  return {
    root,
    setValue(v) {
      currentValue = v || '';
      input.value = labelFor(currentValue);
    },
    setDisabled(d) {
      input.disabled = !!d;
      arrow.disabled = !!d;
      root.classList.toggle('combobox--disabled', !!d);
      if (d) close();
    },
    refresh() {
      if (isOpen) rebuildList();
      else input.value = labelFor(currentValue);
    },
    value() { return currentValue; },
  };
}
