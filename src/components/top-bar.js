import { el, icon } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { createDropdownMenu } from './dropdown-menu.js';
import { saveActiveBuffer, saveAllBuffers, closeBuffer } from '../state/editor.js';
import { editorStore } from '../state/editor.js';
import { openSettings, setCategory, settingsStore, updateSetting } from '../state/settings.js';
import { openCommandPalette } from './command-palette.js';
import { zoomIn, zoomOut, resetZoom } from '../lib/zoom.js';
import { showAlertDialog } from './confirm-dialog.js';
import { createBrandLogo } from './rustic-logo.js';

const ICONS = {
  sidebar: 'M3 3h18v18H3zM9 3v18',
  panel: 'M3 3h18v18H3zM3 15h18',
  secondarySidebar: 'M3 3h18v18H3zM15 3v18',
  minimize: 'M5 12h14',
  maximize: 'M3 3h18v18H3z',
  close: 'M18 6L6 18M6 6l12 12',
};

function createMenuBtn(label, items) {
  const btn = el('button', { class: 'top-bar__menu' }, label);
  const dropdown = createDropdownMenu(items);
  document.body.appendChild(dropdown.element);

  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    const rect = btn.getBoundingClientRect();
    dropdown.show(rect.left, rect.bottom + 2);
  });

  return btn;
}

function createToggleBtn(iconPath, title, stateKey) {
  const btn = el('button', { class: 'top-bar__toggle', title }, icon(iconPath, 14));
  btn.addEventListener('click', () => {
    uiStore.setState({ [stateKey]: !uiStore.getState(stateKey) });
  });
  uiStore.subscribe(stateKey, (val) => btn.classList.toggle('active', val));
  btn.classList.toggle('active', uiStore.getState(stateKey));
  return btn;
}

export function createTopBar() {
  const topBar = el('div', { class: 'top-bar', dataset: { tauriDragRegion: '' } });

  const fileMenu = createMenuBtn('File', [
    { label: 'Open File...', shortcut: 'Ctrl+O', action: async () => {
      try {
        const { open } = await import('@tauri-apps/plugin-dialog');
        const path = await open();
        if (path) {
          const { openFile } = await import('../state/editor.js');
          openFile(path);
        }
      } catch {}
    }},
    { label: 'Open Folder...', action: async () => {
      const { addProject } = await import('../state/workspace.js');
      addProject();
    }},
    { separator: true },
    { label: 'Save', shortcut: 'Ctrl+S', action: () => saveActiveBuffer() },
    { label: 'Save All', shortcut: 'Ctrl+Shift+S', action: () => saveAllBuffers() },
    { separator: true },
    { label: 'Exit', action: async () => {
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        await getCurrentWindow().close();
      } catch {}
    }},
  ]);

  const editMenu = createMenuBtn('Edit', [
    { label: 'Undo', shortcut: 'Ctrl+Z', action: async () => {
      const id = editorStore.getState('activeBufferId');
      if (id) { const { undoEdit } = await import('../lib/tauri-api.js'); undoEdit(id); }
    }},
    { label: 'Redo', shortcut: 'Ctrl+Y', action: async () => {
      const id = editorStore.getState('activeBufferId');
      if (id) { const { redoEdit } = await import('../lib/tauri-api.js'); redoEdit(id); }
    }},
    { separator: true },
    { label: 'Cut', shortcut: 'Ctrl+X', action: () => document.execCommand('cut') },
    { label: 'Copy', shortcut: 'Ctrl+C', action: () => document.execCommand('copy') },
    { label: 'Paste', shortcut: 'Ctrl+V', action: () => document.execCommand('paste') },
    { separator: true },
    { label: 'Find in Files', shortcut: 'Ctrl+Shift+F', action: () => {
      uiStore.setState({ activePanel: 'search', primarySidebarVisible: true });
    }},
  ]);

  const viewMenu = createMenuBtn('View', [
    { label: 'Toggle Sidebar', shortcut: 'Ctrl+B', action: () => {
      uiStore.setState({ primarySidebarVisible: !uiStore.getState('primarySidebarVisible') });
    }},
    { label: 'Toggle Panel', shortcut: 'Ctrl+J', action: () => {
      uiStore.setState({ bottomPanelVisible: !uiStore.getState('bottomPanelVisible') });
    }},
    { label: 'Toggle Secondary Sidebar', action: () => {
      uiStore.setState({ secondarySidebarVisible: !uiStore.getState('secondarySidebarVisible') });
    }},
    { separator: true },
    { label: 'Word Wrap', shortcut: 'Alt+Z', action: () => {
      const s = settingsStore.getState('settings');
      const current = s?.editor?.word_wrap ?? false;
      updateSetting('editor.word_wrap', !current);
    }},
    { label: 'Line Numbers', action: () => {
      const s = settingsStore.getState('settings');
      const current = s?.editor?.line_numbers ?? true;
      updateSetting('editor.line_numbers', !current);
    }},
    { separator: true },
    { label: 'Zoom In', shortcut: 'Ctrl++', action: () => zoomIn() },
    { label: 'Zoom Out', shortcut: 'Ctrl+-', action: () => zoomOut() },
    { label: 'Reset Zoom', shortcut: 'Ctrl+0', action: () => resetZoom() },
    { separator: true },
    { label: 'Command Palette', shortcut: 'Ctrl+Shift+P', action: () => openCommandPalette() },
    { label: 'Quick Open', shortcut: 'Ctrl+P', action: () => openCommandPalette('files') },
  ]);

  const agentMenu = createMenuBtn('Agent', [
    { label: 'Configure Providers', action: () => { setCategory('agent'); openSettings(); } },
    { label: 'MCP Servers', action: () => { setCategory('agent'); openSettings(); } },
    { label: 'Skills', action: () => { setCategory('agent'); openSettings(); } },
    { label: 'Workflows', action: () => { setCategory('agent'); openSettings(); } },
  ]);

  const helpMenu = createMenuBtn('Help', [
    { label: 'Keyboard Shortcuts', action: () => openSettings() },
    { separator: true },
    { label: 'About Rustic', action: () => {
      showAlertDialog('About Rustic', 'Rustic v0.1.0\nA VS Code-inspired agentic IDE\nBuilt with Rust + Tauri 2');
    }},
  ]);

  const logoImg = createBrandLogo();
  const left = el('div', { class: 'top-bar__left' }, [
    el('div', { class: 'top-bar__logo' }, [logoImg]),
    el('div', { class: 'top-bar__menus' }, [fileMenu, editMenu, viewMenu, agentMenu, helpMenu]),
  ]);

  const right = el('div', { class: 'top-bar__right' }, [
    el('div', { class: 'top-bar__toggles' }, [
      createToggleBtn(ICONS.sidebar, 'Toggle Primary Sidebar', 'primarySidebarVisible'),
      createToggleBtn(ICONS.panel, 'Toggle Panel', 'bottomPanelVisible'),
      createToggleBtn(ICONS.secondarySidebar, 'Toggle Secondary Sidebar', 'secondarySidebarVisible'),
    ]),
    el('div', { class: 'top-bar__window-controls' }, [
      createWindowBtn('minimize', ICONS.minimize),
      createWindowBtn('maximize', ICONS.maximize),
      createWindowBtn('close', ICONS.close),
    ]),
  ]);

  topBar.appendChild(left);
  topBar.appendChild(right);

  return topBar;
}

function createWindowBtn(action, iconPath) {
  const btn = el('button', { class: `top-bar__window-btn top-bar__window-btn--${action}` }, icon(iconPath, 14));
  btn.addEventListener('click', async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      const win = getCurrentWindow();
      if (action === 'minimize') await win.minimize();
      else if (action === 'maximize') await win.toggleMaximize();
      else if (action === 'close') await win.close();
    } catch {}
  });
  return btn;
}
