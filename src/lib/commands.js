// Central command registry — single source of truth for every keyboard-
// addressable action in the app. The Shortcuts settings panel lists these,
// and `keybinding-bridge.jsx` dispatches them when the user's bound key
// fires.
//
// Each command has:
//   id          — stable string used in settings.keybindings[].command
//   label       — human-readable name shown in Settings
//   group       — section header in the Settings list (Editor / View / …)
//   defaultKey  — built-in shortcut (or null to mean "click to assign")
//   run         — async function executed when the command is invoked.
//                 Read state lazily inside `run` — the registry is built once
//                 at module load.

import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useExplorer } from '@/state/explorer';
import { useEditor } from '@/state/editor';
import { useSettings } from '@/state/settings';
import { useTerminal } from '@/state/terminal';
import { openCommandPalette, openFilePalette } from '@/components/command-palette';
import { TERMINAL_PICKER_EVENT } from '@/components/terminal-project-picker';
import { formatActiveEditor, saveActiveEditor, toggleCommentActiveEditor } from '@/lib/active-editor';

async function newTerminal() {
  const { projects } = useExplorer.getState();
  // With at least one project open, prompt the user to pick which one to
  // spawn the terminal in (their cwd determines git/lint/dev-script context).
  // The picker also offers a "no project" fallback. With no projects at all,
  // skip the prompt and just open a plain shell so Ctrl+Shift+~ still works
  // before any workspace is opened.
  if (projects.length > 0) {
    window.dispatchEvent(new CustomEvent(TERMINAL_PICKER_EVENT));
    return;
  }
  const info = await useTerminal.getState().createTerminal({ label: 'shell' });
  const title = info.pid != null ? `shell • ${info.pid}` : 'shell';
  useEditor.getState().openTerminal(info.id, title);
}

function cycleTab(delta) {
  const { groups, activeGroupId, setActiveInGroup } = useEditor.getState();
  const group = (groups ?? []).find((g) => g.id === activeGroupId);
  if (!group || !group.tabs?.length) return;
  const idx = group.tabs.findIndex((t) => t.id === group.activeId);
  const next = group.tabs[(idx + delta + group.tabs.length) % group.tabs.length];
  if (next) setActiveInGroup?.(next.id, group.id);
}

function toggleWordWrap() {
  const cur = useSettings.getState().settings;
  if (!cur) return;
  useSettings.getState().update({
    editor: { ...cur.editor, word_wrap: !cur.editor.word_wrap },
  });
}

function dispatchKey(init) {
  // Re-emit a synthetic keystroke for handlers that listen at window level
  // (e.g. useUiZoom for Ctrl+0/=/-, ShortcutCheatsheet for Ctrl+/). bubbles
  // is on so window listeners see it; the bridge skips synthetic events to
  // avoid an infinite loop.
  const ev = new KeyboardEvent('keydown', { ...init, bubbles: true, cancelable: true });
  ev.__rusticSynthetic = true;
  document.dispatchEvent(ev);
}

export const COMMANDS = [
  // ── EDITOR ────────────────────────────────────────────────────────────
  { id: 'editor.nextTab',         label: 'Open Next Tab',     group: 'Editor', defaultKey: 'Ctrl+Tab',       run: () => cycleTab(1) },
  { id: 'editor.prevTab',         label: 'Open Previous Tab', group: 'Editor', defaultKey: 'Ctrl+Shift+Tab', run: () => cycleTab(-1) },
  { id: 'editor.toggleWordWrap',  label: 'Toggle Word Wrap',  group: 'Editor', defaultKey: 'Alt+Z',          run: toggleWordWrap },
  { id: 'editor.formatDocument',  label: 'Format Document',   group: 'Editor', defaultKey: 'Alt+Shift+F',    run: () => formatActiveEditor() },
  { id: 'editor.toggleComment',   label: 'Toggle Comment',    group: 'Editor', defaultKey: 'Ctrl+/',        run: () => toggleCommentActiveEditor() },

  // ── EXPLORER ──────────────────────────────────────────────────────────
  { id: 'explorer.copy',           label: 'Copy',             group: 'Explorer', defaultKey: 'Ctrl+C', run: null },
  { id: 'explorer.cut',            label: 'Cut',              group: 'Explorer', defaultKey: 'Ctrl+X', run: null },
  { id: 'explorer.deleteSelected', label: 'Delete Selected',  group: 'Explorer', defaultKey: 'Delete', run: () => window.dispatchEvent(new CustomEvent('rustic:explorer-delete')) },
  { id: 'explorer.paste',          label: 'Paste',            group: 'Explorer', defaultKey: 'Ctrl+V', run: null },
  { id: 'explorer.rename',         label: 'Rename',           group: 'Explorer', defaultKey: 'F2',     run: () => window.dispatchEvent(new CustomEvent('rustic:explorer-rename')) },

  // ── FILE ──────────────────────────────────────────────────────────────
  { id: 'file.save', label: 'Save File',  group: 'File', defaultKey: 'Ctrl+S', run: () => saveActiveEditor() },
  { id: 'file.new',  label: 'New File',   group: 'File', defaultKey: 'Ctrl+N', run: () => useEditor.getState().openScratch('Untitled', 'plaintext') },

  // ── HELP ──────────────────────────────────────────────────────────────
  { id: 'onboarding.show', label: 'Run Setup Wizard',     group: 'Help', defaultKey: null,    run: () => window.dispatchEvent(new CustomEvent('rustic:open-onboarding')) },
  { id: 'help.showKeyboardShortcuts', label: 'Show Keyboard Shortcuts', group: 'Help', defaultKey: '\\', run: () => dispatchKey({ key: '\\' }) },

  // ── PREFERENCES ───────────────────────────────────────────────────────
  { id: 'settings.show',        label: 'Open Settings',        group: 'Preferences', defaultKey: 'Ctrl+,',       run: () => useLayout.getState().openSettings() },
  { id: 'quickOpen.show',       label: 'Quick Open File',      group: 'Preferences', defaultKey: 'Ctrl+P',       run: () => openFilePalette() },
  { id: 'commandPalette.show',  label: 'Show Command Palette', group: 'Preferences', defaultKey: 'Ctrl+Shift+P', run: () => openCommandPalette() },

  // ── TERMINAL ──────────────────────────────────────────────────────────
  { id: 'terminal.new',    label: 'New Terminal',    group: 'Terminal', defaultKey: 'Ctrl+`', run: newTerminal },
  // Toggle binding intentionally left blank — `view.togglePanel` (Ctrl+J)
  // already does the same thing, and Ctrl+` is now claimed by terminal.new
  // (with project picker).
  { id: 'terminal.toggle', label: 'Toggle Terminal', group: 'Terminal', defaultKey: null,     run: () => useLayout.getState().toggleBottomPanel() },

  // ── VIEW ──────────────────────────────────────────────────────────────
  { id: 'view.zoomReset',             label: 'Reset Zoom',              group: 'View', defaultKey: 'Ctrl+0', run: () => dispatchKey({ key: '0', ctrlKey: true }) },
  { id: 'view.showAgent',             label: 'Show Agent',              group: 'View', defaultKey: null,     run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.AGENT) },
  { id: 'view.showExplorer',          label: 'Show Explorer',           group: 'View', defaultKey: null,     run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.EXPLORER) },
  { id: 'view.showSearch',            label: 'Show Search',             group: 'View', defaultKey: null,     run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.SEARCH) },
  { id: 'view.showSourceControl',     label: 'Show Source Control',     group: 'View', defaultKey: null,     run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.SCM) },
  { id: 'view.togglePanel',           label: 'Toggle Bottom Panel',     group: 'View', defaultKey: 'Ctrl+J', run: () => useLayout.getState().toggleBottomPanel() },
  { id: 'view.toggleSecondarySidebar',label: 'Toggle Secondary Sidebar',group: 'View', defaultKey: null,     run: null },
  { id: 'view.toggleSidebar',         label: 'Toggle Sidebar',          group: 'View', defaultKey: 'Ctrl+B', run: () => useLayout.getState().toggleSidebar() },
  { id: 'view.zoomIn',                label: 'Zoom In',                 group: 'View', defaultKey: 'Ctrl+=', run: () => dispatchKey({ key: '=', ctrlKey: true }) },
  { id: 'view.zoomOut',               label: 'Zoom Out',                group: 'View', defaultKey: 'Ctrl+-', run: () => dispatchKey({ key: '-', ctrlKey: true }) },
];

export const COMMAND_BY_ID = Object.fromEntries(COMMANDS.map((c) => [c.id, c]));

// ── Key normalisation ────────────────────────────────────────────────────
//
// Canonical form is lowercase modifier list joined by `+`, with modifiers
// always in `ctrl+alt+shift+meta+<key>` order regardless of how the user
// pressed them or wrote them in settings. This is what we compare on and
// also what gets persisted.

const MOD_ORDER = ['ctrl', 'alt', 'shift', 'meta'];

const KEY_ALIASES = {
  esc: 'escape',
  ins: 'insert',
  del: 'delete',
  return: 'enter',
  space: ' ',
  spacebar: ' ',
  cmd: 'meta',
  command: 'meta',
  win: 'meta',
  control: 'ctrl',
  option: 'alt',
};

function normalizePart(raw) {
  const p = raw.trim().toLowerCase();
  return KEY_ALIASES[p] ?? p;
}

// Turn "Ctrl+Shift+P" / "shift+ctrl+p" / "cmd+,"  → "ctrl+shift+p"
export function normalizeKey(combo) {
  if (!combo) return '';
  const parts = combo.split('+').map(normalizePart).filter(Boolean);
  const mods = new Set();
  let base = '';
  for (const p of parts) {
    if (MOD_ORDER.includes(p)) mods.add(p);
    else base = p === ' ' ? 'space' : p;
  }
  const ordered = MOD_ORDER.filter((m) => mods.has(m));
  return base ? [...ordered, base].join('+') : ordered.join('+');
}

const DISPLAY_OVERRIDES = {
  ctrl: 'Ctrl',
  alt: 'Alt',
  shift: 'Shift',
  meta: navigator.platform?.toLowerCase().includes('mac') ? '⌘' : 'Meta',
  escape: 'Esc',
  arrowup: '↑',
  arrowdown: '↓',
  arrowleft: '←',
  arrowright: '→',
  space: 'Space',
  enter: 'Enter',
  tab: 'Tab',
  backspace: 'Backspace',
  delete: 'Delete',
};

function displayPart(p) {
  if (DISPLAY_OVERRIDES[p]) return DISPLAY_OVERRIDES[p];
  if (p.length === 1) return p.toUpperCase();
  if (/^f\d+$/.test(p)) return p.toUpperCase();
  return p.charAt(0).toUpperCase() + p.slice(1);
}

export function displayKey(combo) {
  const norm = normalizeKey(combo);
  if (!norm) return '';
  return norm.split('+').map(displayPart).join('+');
}

// Build the canonical key string from a KeyboardEvent. Returns '' if the
// event is just a bare modifier press (Ctrl on its own etc.).
export function eventToKey(e) {
  const k = (e.key || '').toLowerCase();
  if (['control', 'shift', 'alt', 'meta'].includes(k)) return '';

  const parts = [];
  if (e.ctrlKey)  parts.push('ctrl');
  if (e.altKey)   parts.push('alt');
  if (e.shiftKey) parts.push('shift');
  if (e.metaKey)  parts.push('meta');

  let base = k;
  if (base === ' ') base = 'space';
  if (base === '+') base = 'plus';  // avoid colliding with the separator
  parts.push(base);

  return parts.join('+');
}

// Build the effective {key → command-id} map by overlaying user bindings on
// top of the defaults. Multiple commands can share a key (we keep the
// first; conflicts surface in the UI).
export function buildKeyMap(userBindings) {
  const map = new Map();
  for (const cmd of COMMANDS) {
    if (cmd.defaultKey) map.set(normalizeKey(cmd.defaultKey), cmd.id);
  }
  // Drop any default mapping for commands the user has rebound, then apply
  // the user's chosen keys.
  const rebound = new Set((userBindings ?? []).map((b) => b.command));
  for (const [k, id] of [...map.entries()]) {
    if (rebound.has(id)) map.delete(k);
  }
  for (const b of userBindings ?? []) {
    if (!b.key) continue;
    map.set(normalizeKey(b.key), b.command);
  }
  return map;
}

// Effective key for a command (user override beats default).
export function effectiveKey(commandId, userBindings) {
  const override = (userBindings ?? []).find((b) => b.command === commandId);
  if (override) return override.key || null;
  return COMMAND_BY_ID[commandId]?.defaultKey ?? null;
}

// Heuristic — when we're inside an input/textarea/editor, only fire if the
// shortcut uses a non-trivial modifier so we don't intercept typing.
export function isTypingTarget(target) {
  if (!target) return false;
  const tag = (target.tagName || '').toLowerCase();
  if (tag === 'input' || tag === 'textarea' || target.isContentEditable) return true;
  // Any element inside a Monaco editor counts as "typing" so single-key shortcuts
  // (Delete, F2, plain letters) never fire while the user is editing. This used to
  // match only `.monaco-editor .inputarea`, but Monaco routes keydown through several
  // focus states (editor root, view-lines, widgets) where the target isn't the
  // textarea — so Delete leaked through and deleted the selected FILE instead of a
  // character. Modified shortcuts (Ctrl+…) still fire via the caller's modifier check.
  if (target.closest?.('.monaco-editor')) return true;
  if (target.closest?.('.xterm-helper-textarea, .xterm')) return true;
  // Fallback: the key event can target an ancestor while focus actually lives on the
  // Monaco textarea — consult the genuinely-focused element too.
  const active = typeof document !== 'undefined' ? document.activeElement : null;
  if (active && active !== target) {
    const atag = (active.tagName || '').toLowerCase();
    if (atag === 'input' || atag === 'textarea' || active.isContentEditable) return true;
    if (active.closest?.('.monaco-editor, .xterm-helper-textarea, .xterm')) return true;
  }
  return false;
}
