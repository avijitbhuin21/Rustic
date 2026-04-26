// Keybinding dispatcher: maps key combos to command ids and fires them on
// keydown. Components register commands in the command registry; this module
// owns the key→command resolution and the global keydown listener.
//
// The effective binding set is `defaults overlaid with user overrides`. The
// override list is what gets persisted via `settings.keybindings`; defaults
// stay in code so a fresh install Just Works without seed data.

import { executeCommand, getCommand } from './commands.js';

// ── Defaults ──────────────────────────────────────────────────────────────
// Each entry: { key, command, when? } — `key` is normalized lowercase combo
// using '+' as separator (e.g. 'ctrl+shift+p', 'f2', 'alt+z').
export const DEFAULT_BINDINGS = [
  // View
  { key: 'ctrl+b', command: 'view.toggleSidebar' },
  { key: 'ctrl+j', command: 'view.togglePanel' },
  // Ctrl+` mirrors VS Code: toggle the integrated terminal panel. If no
  // terminals exist yet, this creates one — otherwise it just shows/hides
  // the bottom panel that hosts them.
  { key: 'ctrl+`', command: 'terminal.toggle' },
  // 'ctrl+=' fires for both Ctrl+= and Ctrl+Shift+= (the latter being
  // physical "Ctrl+Plus" on US keyboards) — Shift on the Equal/Minus keys
  // is folded out in eventToCombo via e.code, matching VS Code's behavior.
  { key: 'ctrl+=', command: 'view.zoomIn' },
  { key: 'ctrl+-', command: 'view.zoomOut' },
  { key: 'ctrl+0', command: 'view.zoomReset' },
  // Editor
  { key: 'alt+z', command: 'editor.toggleWordWrap' },
  // Settings / palette
  { key: 'ctrl+,', command: 'settings.show' },
  { key: 'ctrl+p', command: 'quickOpen.show' },
  { key: 'ctrl+shift+p', command: 'commandPalette.show' },
  // Explorer
  { key: 'f2', command: 'explorer.rename', when: 'explorerFocus' },
  { key: 'delete', command: 'explorer.deleteSelected', when: 'explorerFocus' },
  // Clipboard ops in the explorer mirror the OS file-manager. The
  // `explorerFocus` when-clause keeps these from hijacking Ctrl+C/X/V
  // inside the editor, terminal, chat input, etc.
  { key: 'ctrl+c', command: 'explorer.copy', when: 'explorerFocus' },
  { key: 'ctrl+x', command: 'explorer.cut', when: 'explorerFocus' },
  { key: 'ctrl+v', command: 'explorer.paste', when: 'explorerFocus' },
];


// ── State ─────────────────────────────────────────────────────────────────

// Active resolved bindings: combo string -> array of { command, when }
// (array because the same combo can resolve to different commands under
// different `when` contexts — first match wins at dispatch time)
let activeBindings = new Map();

// User overrides (persisted via settings.keybindings). When a user edits a
// shortcut from the settings UI we replace the override for that command;
// when they reset, we drop the override.
let userOverrides = []; // [{ key, command, when? }]

// `when` clause checkers — components register their own. e.g. file-tree
// registers 'explorerFocus'.
const whenCheckers = new Map();

// Track whether the listener has been installed so HMR / hot reload don't
// double-bind it.
let listenerInstalled = false;

// ── Key normalization ─────────────────────────────────────────────────────

const MODIFIER_ORDER = ['ctrl', 'shift', 'alt', 'meta'];

// Punctuation/symbol keys whose KeyboardEvent.key changes when Shift is held
// (e.g. '=' becomes '+', '-' becomes '_'). For these we resolve by e.code so
// Ctrl+= and Ctrl+Shift+= collapse to the same combo — matches VS Code.
// Numpad variants are aliased to the same canonical key.
const SHIFT_INSENSITIVE_CODES = {
  Equal: '=',
  Minus: '-',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/',
  Backquote: '`',
  NumpadAdd: '=',
  NumpadSubtract: '-',
};

/**
 * Convert a human key combo string into the canonical form.
 * 'Ctrl+Shift+P' → 'ctrl+shift+p'. Modifier order is normalized so 'shift+ctrl+p'
 * and 'ctrl+shift+p' both resolve to the same key.
 *
 * Handles literal '+' as a key: 'ctrl++' → modifiers ctrl, key '+'. Without
 * special-casing, splitting on '+' would drop the literal + entirely.
 */
export function normalizeKey(combo) {
  if (!combo) return '';
  let working = combo.toLowerCase().trim();
  let trailingPlusKey = false;
  // 'ctrl++' or 'shift+ctrl++' — last '+' is the literal key, the one before
  // it is the separator.
  if (working.length >= 2 && working.endsWith('+') && working[working.length - 2] === '+') {
    trailingPlusKey = true;
    working = working.slice(0, -1);
  } else if (working === '+') {
    return '+';
  }

  const parts = working.split(/\s*\+\s*/).filter(Boolean);
  const mods = new Set();
  let key = '';
  for (const p of parts) {
    if (p === 'cmd' || p === 'command') mods.add('meta');
    else if (p === 'control') mods.add('ctrl');
    else if (p === 'option') mods.add('alt');
    else if (MODIFIER_ORDER.includes(p)) mods.add(p);
    else key = p;
  }
  if (trailingPlusKey) key = '+';
  const ordered = MODIFIER_ORDER.filter(m => mods.has(m));
  if (key) ordered.push(key);
  return ordered.join('+');
}

/** Convert a KeyboardEvent into the canonical combo form. */
export function eventToCombo(e) {
  // Skip pure modifier-only presses (e.g. just Ctrl).
  if (e.key === 'Control' || e.key === 'Shift' || e.key === 'Alt' || e.key === 'Meta') {
    return '';
  }

  // For shift-insensitive symbol keys, resolve via e.code and drop Shift from
  // modifiers — so Ctrl+= and Ctrl+Shift+= (the physical "Ctrl+Plus") both
  // produce 'ctrl+='.
  const codeKey = SHIFT_INSENSITIVE_CODES[e.code];

  const parts = [];
  if (e.ctrlKey) parts.push('ctrl');
  if (e.shiftKey && !codeKey) parts.push('shift');
  if (e.altKey) parts.push('alt');
  if (e.metaKey) parts.push('meta');

  let key;
  if (codeKey) {
    key = codeKey;
  } else if (e.key.length === 1) {
    key = e.key.toLowerCase();
  } else {
    // Function keys, arrows, etc. — already strings like 'F2', 'ArrowDown'.
    key = e.key.toLowerCase();
  }
  // Normalize a few common synonyms.
  if (key === ' ') key = 'space';
  if (key === 'esc') key = 'escape';
  if (key === 'del') key = 'delete';

  parts.push(key);
  return parts.join('+');
}

// ── Effective binding resolution ──────────────────────────────────────────

/**
 * Return the effective binding list (defaults with user overrides applied).
 * Defaults are dropped when:
 *   (a) the command was overridden — its old default would still fire otherwise
 *   (b) the key was overridden — same key can't drive two commands
 * User overrides come last so they're appended fresh.
 */
export function getEffectiveBindings() {
  const overriddenCommands = new Set(userOverrides.map(o => o.command));
  const overriddenKeys = new Set(
    userOverrides.map(o => normalizeKey(o.key)).filter(Boolean),
  );
  const result = [];
  for (const b of DEFAULT_BINDINGS) {
    if (overriddenCommands.has(b.command)) continue;
    if (overriddenKeys.has(normalizeKey(b.key))) continue;
    result.push({ ...b, source: 'default' });
  }
  for (const o of userOverrides) {
    if (!o.key) continue;
    result.push({ ...o, source: 'user' });
  }
  return result;
}

/** Get the binding currently bound to a specific command (effective). */
export function getBindingForCommand(commandId) {
  const eff = getEffectiveBindings();
  return eff.find(b => b.command === commandId) || null;
}

/** Get the original default binding for a command (used by "Reset" buttons). */
export function getDefaultBindingForCommand(commandId) {
  return DEFAULT_BINDINGS.find(b => b.command === commandId) || null;
}

/** Replace the user override list and rebuild the dispatch table. */
export function setOverrides(overrides) {
  userOverrides = (overrides || [])
    .filter(o => o && o.command)
    .map(o => ({
      command: o.command,
      key: normalizeKey(o.key),
      when: o.when || undefined,
    }));
  rebuildDispatchTable();
}

export function getOverrides() {
  return userOverrides.map(o => ({ ...o }));
}

/**
 * Set/clear an override for a single command. Pass `null` for `key` to remove
 * the override and revert to the default. Returns the updated override list
 * so callers can persist it.
 */
export function setBinding(commandId, key, when) {
  userOverrides = userOverrides.filter(o => o.command !== commandId);
  if (key) {
    userOverrides.push({
      command: commandId,
      key: normalizeKey(key),
      when: when || undefined,
    });
  }
  rebuildDispatchTable();
  return getOverrides();
}

/** Find which command a given key combo currently resolves to (for conflict checks). */
export function findCommandForKey(combo) {
  const normalized = normalizeKey(combo);
  const candidates = activeBindings.get(normalized);
  if (!candidates || candidates.length === 0) return null;
  return candidates[0].command;
}

function rebuildDispatchTable() {
  activeBindings = new Map();
  for (const b of getEffectiveBindings()) {
    const key = normalizeKey(b.key);
    if (!key) continue;
    if (!activeBindings.has(key)) activeBindings.set(key, []);
    activeBindings.get(key).push({ command: b.command, when: b.when });
  }
}

// ── When-clause evaluation ────────────────────────────────────────────────

export function registerWhen(name, checker) {
  whenCheckers.set(name, checker);
}

function evaluateWhen(when) {
  if (!when) return true;
  const checker = whenCheckers.get(when);
  return checker ? !!checker() : false;
}

// ── Input-focus suppression ───────────────────────────────────────────────
// Bare keys (F2, Delete, etc.) shouldn't trigger commands while the user
// is typing in a text field. Modifier-prefixed combos (Ctrl+B, Alt+Z) are
// always allowed unless the command opts out.

function isTypingInInput() {
  const a = document.activeElement;
  if (!a) return false;
  const tag = a.tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA') return true;
  if (a.isContentEditable) return true;
  // CodeMirror focuses internal contenteditable elements — caught above.
  return false;
}

function comboHasModifier(combo) {
  return combo.startsWith('ctrl+') || combo.startsWith('alt+') ||
         combo.startsWith('meta+') || combo.includes('+ctrl+') ||
         combo.includes('+alt+') || combo.includes('+meta+');
}

// ── Listener ──────────────────────────────────────────────────────────────

export function installKeybindingListener() {
  if (listenerInstalled) return;
  listenerInstalled = true;
  rebuildDispatchTable();

  document.addEventListener('keydown', (e) => {
    const combo = eventToCombo(e);
    if (!combo) return;
    const candidates = activeBindings.get(combo);
    if (!candidates || candidates.length === 0) return;

    const typing = isTypingInInput();
    const hasMod = comboHasModifier(combo);

    for (const { command, when } of candidates) {
      if (!evaluateWhen(when)) continue;
      const cmd = getCommand(command);
      if (!cmd) continue;
      // Suppress bare-key bindings while typing unless the command opts in
      // OR has a `when` clause. The `when` clause IS the context check —
      // if it evaluated true, the binding's owner (e.g. the explorer with
      // a selected file) has taken responsibility for the context, so we
      // shouldn't second-guess it just because focus happens to be in the
      // editor textarea. Without this, F2 to rename never fires after a
      // file click moves focus into the editor.
      if (typing && !hasMod && !cmd.allowInInput && !when) continue;
      e.preventDefault();
      executeCommand(command);
      return;
    }
  });
}

/** Convert a normalized combo into a human-readable label for the UI. */
export function formatCombo(combo) {
  if (!combo) return '';
  return combo
    .split('+')
    .map(p => p.charAt(0).toUpperCase() + p.slice(1))
    .join('+');
}
