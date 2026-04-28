// Shortcut cheatsheet overlay. Triggered by Ctrl+/ — renders every
// command currently registered alongside its bound key, grouped by
// category. Read-only quick reference; rebinding still happens in
// Settings → Shortcuts.

import { el, icon } from '../utils/dom.js';
import { getAllCommands } from '../lib/commands.js';
import { DEFAULT_BINDINGS } from '../lib/keybindings.js';
import { settingsStore } from '../state/settings.js';
import { trapFocus } from './confirm-dialog.js';

let activeOverlay = null;

/// Build a map command_id -> display string by merging user-defined
/// overrides on top of DEFAULT_BINDINGS. Multiple bindings for the same
/// command are joined with " or ".
function resolveBindingsForDisplay() {
  const overrides = settingsStore.getState('settings')?.keybindings || [];
  const fromList = (list) => {
    const m = new Map();
    for (const b of list) {
      if (!b?.command || !b?.key) continue;
      const arr = m.get(b.command) || [];
      arr.push(b.key);
      m.set(b.command, arr);
    }
    return m;
  };
  const defaults = fromList(DEFAULT_BINDINGS);
  const userMap = fromList(overrides);
  // User overrides REPLACE defaults for that command — same as the
  // dispatcher's actual behavior. (See keybindings.js: setOverrides.)
  const merged = new Map(defaults);
  for (const [cmd, keys] of userMap.entries()) {
    merged.set(cmd, keys);
  }
  return merged;
}

/// Render a "Ctrl+P" string into a row of styled <kbd> chips.
function renderKbds(comboString) {
  const wrap = el('span', { class: 'shortcut-cheat__keys' });
  const parts = comboString.split('+');
  parts.forEach((part, i) => {
    const text = part === 'ctrl' ? 'Ctrl'
      : part === 'shift' ? 'Shift'
      : part === 'alt' ? 'Alt'
      : part === 'meta' ? '⌘'
      : part.length === 1 ? part.toUpperCase()
      : part.charAt(0).toUpperCase() + part.slice(1);
    wrap.appendChild(el('kbd', { class: 'kbd' }, text));
    if (i < parts.length - 1) {
      wrap.appendChild(el('span', { class: 'shortcut-cheat__plus' }, '+'));
    }
  });
  return wrap;
}

export function showShortcutCheatsheet() {
  if (activeOverlay) {
    closeShortcutCheatsheet();
    return;
  }

  const overlay = el('div', { class: 'modal-overlay shortcut-cheat-overlay' });
  const card = el('div', { class: 'modal-card shortcut-cheat-card' });

  // Header with search
  const header = el('div', { class: 'modal-card__header shortcut-cheat__header' });
  header.appendChild(el('div', { class: 'modal-card__title' }, 'Keyboard shortcuts'));

  const searchInput = el('input', {
    class: 'shortcut-cheat__search',
    type: 'text',
    placeholder: 'Filter…',
    autocomplete: 'off',
    spellcheck: 'false',
  });
  header.appendChild(searchInput);

  const closeBtn = el('button', { class: 'btn btn--ghost btn--xs', title: 'Close' }, '×');
  closeBtn.addEventListener('click', () => closeShortcutCheatsheet());
  header.appendChild(closeBtn);

  card.appendChild(header);

  // Body
  const body = el('div', { class: 'modal-card__body shortcut-cheat__body' });
  card.appendChild(body);

  const footer = el('div', { class: 'modal-card__footer shortcut-cheat__footer' });
  footer.appendChild(el('span', { class: 'shortcut-cheat__hint' },
    'Tip: customize any of these in Settings → Shortcuts.'));
  card.appendChild(footer);

  function rerender(query) {
    body.innerHTML = '';
    const q = (query || '').trim().toLowerCase();
    const bindings = resolveBindingsForDisplay();
    const cmds = getAllCommands();

    // Group by category, only including commands that match the query.
    const groups = new Map();
    for (const cmd of cmds) {
      const keys = bindings.get(cmd.id) || [];
      const match = !q
        || cmd.title.toLowerCase().includes(q)
        || cmd.category.toLowerCase().includes(q)
        || keys.some((k) => k.toLowerCase().includes(q));
      if (!match) continue;
      const list = groups.get(cmd.category) || [];
      list.push({ cmd, keys });
      groups.set(cmd.category, list);
    }

    if (groups.size === 0) {
      body.appendChild(el('div', { class: 'shortcut-cheat__empty' },
        `No shortcuts match "${q}".`));
      return;
    }

    // Render alphabetized categories.
    for (const cat of Array.from(groups.keys()).sort()) {
      const section = el('div', { class: 'shortcut-cheat__section' });
      section.appendChild(el('div', { class: 'shortcut-cheat__category' }, cat));
      const list = el('div', { class: 'shortcut-cheat__list' });
      for (const { cmd, keys } of groups.get(cat)) {
        const row = el('div', { class: 'shortcut-cheat__row' });
        row.appendChild(el('div', { class: 'shortcut-cheat__title' }, cmd.title));
        const keyArea = el('div', { class: 'shortcut-cheat__key-area' });
        if (keys.length === 0) {
          keyArea.appendChild(el('span', { class: 'shortcut-cheat__unbound' }, 'Unbound'));
        } else {
          keys.forEach((k, i) => {
            keyArea.appendChild(renderKbds(k));
            if (i < keys.length - 1) {
              keyArea.appendChild(el('span', { class: 'shortcut-cheat__or' }, 'or'));
            }
          });
        }
        row.appendChild(keyArea);
        list.appendChild(row);
      }
      section.appendChild(list);
      body.appendChild(section);
    }
  }

  rerender('');

  searchInput.addEventListener('input', () => rerender(searchInput.value));
  searchInput.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      closeShortcutCheatsheet();
    }
  });

  function onKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      closeShortcutCheatsheet();
    }
  }

  overlay.appendChild(card);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) closeShortcutCheatsheet();
  });
  document.body.appendChild(overlay);
  document.addEventListener('keydown', onKey);
  const releaseTrap = trapFocus(card);

  setTimeout(() => searchInput.focus(), 0);

  activeOverlay = {
    close: () => {
      releaseTrap();
      document.removeEventListener('keydown', onKey);
      overlay.remove();
      activeOverlay = null;
    },
  };
}

export function closeShortcutCheatsheet() {
  if (activeOverlay) activeOverlay.close();
}

export function isShortcutCheatsheetOpen() {
  return !!activeOverlay;
}
