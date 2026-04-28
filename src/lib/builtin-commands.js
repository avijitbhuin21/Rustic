// Registers built-in app commands with the command registry. Called once
// during app startup, after the modules being driven are imported.
//
// Commands here are the document-level shortcuts that previously had their
// own ad-hoc `addEventListener('keydown', ...)` handlers scattered across
// main.js / command-palette.js. Editor-internal shortcuts (CodeMirror, the
// find-replace widget, etc.) stay scoped to their elements; those don't
// belong in the global dispatcher.

import { registerCommand } from './commands.js';
import { uiStore } from '../state/ui.js';
import { settingsStore, openSettings, updateSetting } from '../state/settings.js';
import { editorStore, saveActiveBuffer } from '../state/editor.js';
import { zoomIn, zoomOut, resetZoom } from './zoom.js';
import * as api from './tauri-api.js';

export function registerBuiltinCommands() {
  // ── View ────────────────────────────────────────────────────────────────
  registerCommand({
    id: 'view.toggleSidebar',
    title: 'Toggle Sidebar',
    category: 'View',
    run: () => uiStore.setState({
      primarySidebarVisible: !uiStore.getState('primarySidebarVisible'),
    }),
  });
  registerCommand({
    id: 'view.togglePanel',
    title: 'Toggle Bottom Panel',
    category: 'View',
    run: () => uiStore.setState({
      bottomPanelVisible: !uiStore.getState('bottomPanelVisible'),
    }),
  });
  registerCommand({
    id: 'view.toggleSecondarySidebar',
    title: 'Toggle Secondary Sidebar',
    category: 'View',
    run: () => uiStore.setState({
      secondarySidebarVisible: !uiStore.getState('secondarySidebarVisible'),
    }),
  });
  registerCommand({
    id: 'view.zoomIn',
    title: 'Zoom In',
    category: 'View',
    allowInInput: true,
    run: () => zoomIn(),
  });
  registerCommand({
    id: 'view.zoomOut',
    title: 'Zoom Out',
    category: 'View',
    allowInInput: true,
    run: () => zoomOut(),
  });
  registerCommand({
    id: 'view.zoomReset',
    title: 'Reset Zoom',
    category: 'View',
    allowInInput: true,
    run: () => resetZoom(),
  });
  registerCommand({
    id: 'view.showExplorer',
    title: 'Show Explorer',
    category: 'View',
    run: () => uiStore.setState({ activePanel: 'explorer', primarySidebarVisible: true }),
  });
  registerCommand({
    id: 'view.showSearch',
    title: 'Show Search',
    category: 'View',
    run: () => uiStore.setState({ activePanel: 'search', primarySidebarVisible: true }),
  });
  registerCommand({
    id: 'view.showSourceControl',
    title: 'Show Source Control',
    category: 'View',
    run: () => uiStore.setState({ activePanel: 'git', primarySidebarVisible: true }),
  });
  registerCommand({
    id: 'view.showAgent',
    title: 'Show Agent',
    category: 'View',
    run: () => uiStore.setState({ activePanel: 'agent', primarySidebarVisible: true }),
  });

  // ── Editor ──────────────────────────────────────────────────────────────
  registerCommand({
    id: 'editor.toggleWordWrap',
    title: 'Toggle Word Wrap',
    category: 'Editor',
    run: () => {
      const s = settingsStore.getState('settings');
      const current = s?.editor?.word_wrap ?? false;
      updateSetting('editor.word_wrap', !current);
    },
  });
  registerCommand({
    id: 'editor.formatDocument',
    title: 'Format Document',
    category: 'Editor',
    run: async () => {
      const id = editorStore.getState('activeBufferId');
      if (id) await api.formatDocument(id);
    },
  });

  // ── File ────────────────────────────────────────────────────────────────
  registerCommand({
    id: 'file.save',
    title: 'Save File',
    category: 'File',
    allowInInput: true,
    run: () => saveActiveBuffer(),
  });

  // ── Settings & palette ──────────────────────────────────────────────────
  registerCommand({
    id: 'settings.show',
    title: 'Open Settings',
    category: 'Preferences',
    allowInInput: true,
    run: () => openSettings(),
  });
  registerCommand({
    id: 'commandPalette.show',
    title: 'Show Command Palette',
    category: 'Preferences',
    allowInInput: true,
    run: async () => {
      const { openCommandPalette } = await import('../components/command-palette.js');
      openCommandPalette('commands');
    },
  });
  registerCommand({
    id: 'quickOpen.show',
    title: 'Quick Open File',
    category: 'Preferences',
    allowInInput: true,
    run: async () => {
      const { openCommandPalette } = await import('../components/command-palette.js');
      openCommandPalette('files');
    },
  });
  registerCommand({
    id: 'onboarding.show',
    title: 'Run Setup Wizard',
    category: 'Help',
    allowInInput: true,
    run: async () => {
      const { showOnboardingWizard } = await import('../components/onboarding/onboarding-wizard.js');
      showOnboardingWizard({ force: true });
    },
  });
  registerCommand({
    id: 'help.showKeyboardShortcuts',
    title: 'Show Keyboard Shortcuts',
    category: 'Help',
    allowInInput: true,
    run: async () => {
      const { showShortcutCheatsheet } = await import('../components/shortcut-cheatsheet.js');
      showShortcutCheatsheet();
    },
  });

  // ── Terminal ────────────────────────────────────────────────────────────
  registerCommand({
    id: 'terminal.new',
    title: 'New Terminal',
    category: 'Terminal',
    run: async () => {
      const { createTerminal } = await import('../state/terminal.js');
      createTerminal();
    },
  });
  // VS Code-style toggle: Ctrl+` opens the terminal panel, hides it if
  // already visible, and spins up a fresh terminal session if there are
  // no existing ones (otherwise the panel would show empty tabs).
  registerCommand({
    id: 'terminal.toggle',
    title: 'Toggle Terminal',
    category: 'Terminal',
    allowInInput: true,
    run: async () => {
      const { terminalStore, createTerminal } = await import('../state/terminal.js');
      const visible = uiStore.getState('bottomPanelVisible');
      const sessions = terminalStore.getState('sessions');
      if (visible) {
        // Already showing — hide it.
        uiStore.setState({ bottomPanelVisible: false });
        return;
      }
      if (sessions.length === 0) {
        // No terminals exist yet — createTerminal() flips the panel visible
        // for us once the session is registered.
        await createTerminal();
      } else {
        uiStore.setState({ bottomPanelVisible: true });
      }
    },
  });
}
