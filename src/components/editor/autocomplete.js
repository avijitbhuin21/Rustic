import { el } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

/**
 * Autocomplete popup overlay for the editor.
 * Call show() with buffer info + cursor position; call hide() to dismiss.
 */
export function createAutocomplete(onAccept) {
  const popup = el('div', { class: 'autocomplete-popup' });
  popup.style.display = 'none';

  let items = [];
  let selectedIndex = 0;
  let visible = false;

  function render() {
    popup.innerHTML = '';
    if (items.length === 0) {
      hide();
      return;
    }
    items.forEach((item, i) => {
      const row = el('div', {
        class: `autocomplete-item ${i === selectedIndex ? 'autocomplete-item--selected' : ''}`,
      });
      const kindBadge = el('span', { class: `autocomplete-kind autocomplete-kind--${item.kind.toLowerCase()}` },
        item.kind.charAt(0)
      );
      const label = el('span', { class: 'autocomplete-label' }, item.label);
      row.appendChild(kindBadge);
      row.appendChild(label);
      if (item.detail) {
        row.appendChild(el('span', { class: 'autocomplete-detail' }, item.detail));
      }
      row.addEventListener('click', () => accept(i));
      popup.appendChild(row);
    });

    // Scroll selected into view
    const selected = popup.querySelector('.autocomplete-item--selected');
    if (selected) selected.scrollIntoView({ block: 'nearest' });
  }

  function accept(index) {
    const item = items[index];
    if (item && onAccept) {
      onAccept(item.insert_text || item.label);
    }
    hide();
  }

  async function show(bufferId, line, col, x, y) {
    try {
      const result = await api.getCompletions(bufferId, line, col);
      items = result || [];
      selectedIndex = 0;

      if (items.length === 0) {
        hide();
        return;
      }

      popup.style.left = `${x}px`;
      popup.style.top = `${y + 20}px`;
      popup.style.display = 'block';
      visible = true;
      render();
    } catch {
      hide();
    }
  }

  function hide() {
    popup.style.display = 'none';
    visible = false;
    items = [];
    selectedIndex = 0;
  }

  function handleKey(e) {
    if (!visible) return false;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectedIndex = (selectedIndex + 1) % items.length;
      render();
      return true;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectedIndex = (selectedIndex - 1 + items.length) % items.length;
      render();
      return true;
    }
    if (e.key === 'Enter' || e.key === 'Tab') {
      e.preventDefault();
      accept(selectedIndex);
      return true;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      hide();
      return true;
    }
    return false;
  }

  return { element: popup, show, hide, handleKey, isVisible: () => visible };
}
