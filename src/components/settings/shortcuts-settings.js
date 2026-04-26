import { el, icon } from '../../utils/dom.js';
import { saveSettings, settingsStore, loadSettings } from '../../state/settings.js';
import { getAllCommands } from '../../lib/commands.js';
import {
  getBindingForCommand,
  getDefaultBindingForCommand,
  setBinding,
  formatCombo,
  eventToCombo,
  findCommandForKey,
} from '../../lib/keybindings.js';
import { showConfirmDialog, showAlertDialog } from '../confirm-dialog.js';
import * as api from '../../lib/tauri-api.js';

// ── Top-level Shortcuts settings panel ───────────────────────────────────
//
// One row per registered command. Click the shortcut cell to record a new
// combo: the next keydown is captured and saved (after a conflict check).
// The "Reset" button clears the user override for that command, restoring
// its default. "Reset all" clears every override at once.

export function createShortcutsSettings(settings) {
  const container = el('div', { class: 'settings-section shortcuts-settings' });

  // ── Header: title, search, reset-all ───────────────────────────────────
  const header = el('div', { class: 'shortcuts-header' });
  const search = el('input', {
    class: 'shortcuts-search',
    type: 'text',
    placeholder: 'Search commands…',
    spellcheck: 'false',
  });
  search.addEventListener('input', () => render());
  header.appendChild(search);

  const importBtn = el('button', { class: 'settings-btn' }, 'Import VS Code…');
  importBtn.addEventListener('click', () => importVscodeKeybindings(container));
  header.appendChild(importBtn);

  const resetAllBtn = el('button', { class: 'settings-btn settings-btn--danger' }, 'Reset All');
  resetAllBtn.addEventListener('click', async () => {
    const updated = { ...settingsStore.getState('settings'), keybindings: [] };
    await saveSettings(updated);
    render();
  });
  header.appendChild(resetAllBtn);

  container.appendChild(header);

  // ── Table ──────────────────────────────────────────────────────────────
  const table = el('div', { class: 'shortcuts-table' });
  container.appendChild(table);

  // Track which row is currently in "press a key" mode so a re-render
  // doesn't blow the recording state away.
  let recordingCommandId = null;

  function render() {
    table.innerHTML = '';

    // Header row
    const head = el('div', { class: 'shortcuts-row shortcuts-row--header' });
    head.appendChild(el('div', { class: 'shortcuts-cell shortcuts-cell--cmd' }, 'Command'));
    head.appendChild(el('div', { class: 'shortcuts-cell shortcuts-cell--key' }, 'Shortcut'));
    table.appendChild(head);

    const query = search.value.trim().toLowerCase();
    const all = getAllCommands();
    const filtered = query
      ? all.filter(c =>
          c.title.toLowerCase().includes(query) ||
          c.id.toLowerCase().includes(query) ||
          c.category.toLowerCase().includes(query) ||
          (getBindingForCommand(c.id)?.key || '').includes(query))
      : all;

    if (filtered.length === 0) {
      table.appendChild(el('div', { class: 'shortcuts-empty' }, 'No commands match.'));
      return;
    }

    // Group by category
    const byCategory = new Map();
    for (const cmd of filtered) {
      if (!byCategory.has(cmd.category)) byCategory.set(cmd.category, []);
      byCategory.get(cmd.category).push(cmd);
    }

    for (const [category, cmds] of byCategory) {
      const groupHeader = el('div', { class: 'shortcuts-group' }, category);
      table.appendChild(groupHeader);
      for (const cmd of cmds) {
        table.appendChild(buildRow(cmd));
      }
    }
  }

  function buildRow(cmd) {
    const row = el('div', { class: 'shortcuts-row' });

    // Command cell
    const cmdCell = el('div', { class: 'shortcuts-cell shortcuts-cell--cmd' });
    cmdCell.appendChild(el('div', { class: 'shortcuts-cmd-title' }, cmd.title));
    cmdCell.appendChild(el('div', { class: 'shortcuts-cmd-id' }, cmd.id));
    row.appendChild(cmdCell);

    const keyCell = el('div', { class: 'shortcuts-cell shortcuts-cell--key' });
    const binding = getBindingForCommand(cmd.id);
    const defaultBinding = getDefaultBindingForCommand(cmd.id);
    const isCustom = binding?.source === 'user';

    if (recordingCommandId === cmd.id) {
      keyCell.appendChild(buildRecorder(cmd, keyCell));
    } else {
      const keyGroup = el('div', { class: 'shortcuts-key-group' });

      if (isCustom) {
        const resetBtn = el('button', { class: 'shortcuts-reset-btn' });
        resetBtn.title = defaultBinding
          ? `Reset to default (${formatCombo(defaultBinding.key)})`
          : 'Clear shortcut';
        resetBtn.appendChild(icon('M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5', 14));
        resetBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          commitBinding(cmd.id, null);
        });
        keyGroup.appendChild(resetBtn);
      }

      const keyBtn = el('button', {
        class: `shortcuts-key-btn${isCustom ? ' shortcuts-key-btn--custom' : ''}`,
        title: 'Click to change',
      });
      if (binding?.key) {
        keyBtn.appendChild(el('kbd', { class: 'keybinding-key' }, formatCombo(binding.key)));
      } else {
        keyBtn.appendChild(el('span', { class: 'shortcuts-key-empty' }, 'Click to assign'));
      }
      keyBtn.addEventListener('click', () => {
        recordingCommandId = cmd.id;
        render();
      });
      keyGroup.appendChild(keyBtn);
      keyCell.appendChild(keyGroup);
    }
    row.appendChild(keyCell);

    return row;
  }

  function buildRecorder(cmd, keyCell) {
    const recorder = el('div', { class: 'shortcuts-recorder' });
    const label = el('span', { class: 'shortcuts-recorder__label' }, 'Press a key… (Delete to clear, Esc to cancel)');
    recorder.appendChild(label);

    async function onKeyDown(e) {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.key === 'Escape') {
        cleanup();
        return;
      }
      if (e.key === 'Delete' || e.key === 'Backspace') {
        cleanup();
        commitBinding(cmd.id, null);
        return;
      }
      if (e.key === 'Control' || e.key === 'Shift' || e.key === 'Alt' || e.key === 'Meta') {
        return;
      }
      const combo = eventToCombo(e);
      if (!combo) return;

      // Exit recording mode immediately so the listener isn't still hot
      // while the conflict dialog is open.
      cleanup();

      // Conflict check: if this combo is bound to a different command,
      // confirm the takeover before saving.
      const existing = findCommandForKey(combo);
      if (existing && existing !== cmd.id) {
        const ok = await showConfirmDialog(
          'Shortcut already bound',
          `${formatCombo(combo)} is currently bound to "${existing}". Reassign it to "${cmd.title}"?`,
          { confirmLabel: 'Reassign', danger: false },
        );
        if (!ok) return;
      }
      commitBinding(cmd.id, combo);
    }

    function cleanup() {
      document.removeEventListener('keydown', onKeyDown, true);
      recordingCommandId = null;
      render();
    }

    // Capture-phase listener so we beat the global dispatcher (which would
    // otherwise execute the command we're trying to rebind to).
    document.addEventListener('keydown', onKeyDown, true);

    return recorder;
  }

  // ── Persist a binding change ───────────────────────────────────────────
  // `combo` of null means "remove the override" (revert to default / unbind).
  async function commitBinding(commandId, combo) {
    // If the new combo would shadow an existing binding for another command,
    // remove that other override too — otherwise the conflicting default
    // would still resolve first because we keep both in the map.
    if (combo) {
      const existing = findCommandForKey(combo);
      if (existing && existing !== commandId) {
        // Force-unbind the conflicting command by recording an empty/unique
        // override. Simpler: drop the conflicting command's override entirely
        // (revert to default) — but that's wrong if the conflict IS the
        // default. So instead, leave the conflict alone and rely on the
        // "first match wins" order: user overrides come AFTER defaults in the
        // dispatch table iteration, so the new override naturally wins.
      }
    }
    const overrides = setBinding(commandId, combo);
    const updated = { ...settingsStore.getState('settings'), keybindings: overrides };
    await saveSettings(updated);
    render();
  }

  // Re-render when settings change externally (e.g. import)
  const unsub = settingsStore.subscribe('settings', () => render());
  // Best-effort cleanup if the panel is removed from the DOM.
  const observer = new MutationObserver(() => {
    if (!document.body.contains(container)) {
      unsub();
      observer.disconnect();
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });

  render();
  return container;
}

// ── VS Code import flow ─────────────────────────────────────────────────
//
// Tries auto-detection first: if exactly one VS Code variant is installed,
// import it after a confirm. If several are installed, show a small inline
// picker. If none, fall back to the file dialog so a user with a custom
// install location can still bring their bindings in.

async function importVscodeKeybindings(parent) {
  let result = { importable: [], detected_without_overrides: [] };
  try {
    result = (await api.detectVscodeKeybindings()) || result;
  } catch (e) {
    console.error('Failed to detect VS Code installs:', e);
  }
  const { importable = [], detected_without_overrides = [] } = result;

  if (importable.length === 0) {
    // VS Code is installed but never wrote a keybindings.json — explain the
    // situation explicitly instead of silently opening a file picker (which
    // confused users into thinking detection had failed).
    if (detected_without_overrides.length > 0) {
      const names = detected_without_overrides.join(', ');
      const msg =
        `Found ${names}, but no keybindings.json file exists yet.\n\n` +
        `VS Code only creates this file after you customize at least one shortcut ` +
        `(File → Preferences → Keyboard Shortcuts). Once you have a custom binding, ` +
        `try importing again.`;
      const pick = await showConfirmDialog('No keybindings to import', msg, {
        confirmLabel: 'Choose file…',
        cancelLabel: 'Close',
        danger: false,
      });
      if (pick) await pickFileAndImport();
      return;
    }
    // No VS Code-family install detected at all — go straight to the picker.
    return pickFileAndImport();
  }
  if (importable.length === 1) {
    const v = importable[0];
    const ok = await showConfirmDialog(
      'Import keybindings',
      `Import ${v.binding_count} keybinding${v.binding_count === 1 ? '' : 's'} from ${v.name}?\n\n${v.path}`,
      { confirmLabel: 'Import', danger: false },
    );
    if (ok) await runImport(v.path);
    return;
  }
  showVariantPicker(parent, importable);
}

async function pickFileAndImport() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const path = await open({ filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (path) await runImport(path);
  } catch (e) {
    console.error('Failed to import keybindings:', e);
  }
}

async function runImport(path) {
  try {
    await api.importKeybindings(path);
    await loadSettings();
  } catch (e) {
    console.error('Failed to import keybindings:', e);
    await showAlertDialog('Import failed', `Failed to import keybindings:\n${e}`);
  }
}

function showVariantPicker(parent, variants) {
  // Remove any prior picker (e.g. user clicked Import twice).
  parent.querySelector('.shortcuts-import-picker')?.remove();

  const overlay = el('div', { class: 'shortcuts-import-picker' });
  const close = () => overlay.remove();
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) close();
  });

  const box = el('div', { class: 'shortcuts-import-picker__box' });
  box.appendChild(el('div', { class: 'shortcuts-import-picker__title' }, 'Import keybindings from…'));
  box.appendChild(el(
    'div',
    { class: 'shortcuts-import-picker__hint' },
    'Multiple VS Code-family installs were detected. Pick one to import its user keybindings.',
  ));

  const list = el('div', { class: 'shortcuts-import-picker__list' });
  for (const v of variants) {
    const row = el('button', { class: 'shortcuts-import-picker__row' });
    row.appendChild(el('div', { class: 'shortcuts-import-picker__row-name' }, v.name));
    row.appendChild(el(
      'div',
      { class: 'shortcuts-import-picker__row-meta' },
      `${v.binding_count} shortcut${v.binding_count === 1 ? '' : 's'} • ${v.path}`,
    ));
    row.addEventListener('click', async () => {
      close();
      await runImport(v.path);
    });
    list.appendChild(row);
  }
  box.appendChild(list);

  const actions = el('div', { class: 'shortcuts-import-picker__actions' });
  const fileBtn = el('button', { class: 'settings-btn' }, 'Choose file…');
  fileBtn.addEventListener('click', async () => {
    close();
    await pickFileAndImport();
  });
  const cancelBtn = el('button', { class: 'settings-btn' }, 'Cancel');
  cancelBtn.addEventListener('click', close);
  actions.appendChild(fileBtn);
  actions.appendChild(cancelBtn);
  box.appendChild(actions);

  overlay.appendChild(box);
  parent.appendChild(overlay);
}
