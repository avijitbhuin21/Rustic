import { el } from '../../utils/dom.js';
import { terminalStore } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';

// We'll dynamically import xterm to handle the case where it might not be available
let Terminal, FitAddon;

async function loadXterm() {
  if (Terminal) return;
  try {
    const xtermMod = await import('xterm');
    const fitMod = await import('@xterm/addon-fit');
    Terminal = xtermMod.Terminal;
    FitAddon = fitMod.FitAddon;

    // Import xterm CSS (Vite handles this)
    await import('xterm/css/xterm.css');
  } catch (e) {
    console.error('Failed to load xterm:', e);
  }
}

// Gruvbox Dark theme for xterm.js
const GRUVBOX_THEME = {
  background: '#282828',
  foreground: '#ebdbb2',
  cursor: '#ebdbb2',
  cursorAccent: '#282828',
  selectionBackground: '#504945',
  black: '#282828',
  red: '#cc241d',
  green: '#98971a',
  yellow: '#d79921',
  blue: '#458588',
  magenta: '#b16286',
  cyan: '#689d6a',
  white: '#a89984',
  brightBlack: '#928374',
  brightRed: '#fb4934',
  brightGreen: '#b8bb26',
  brightYellow: '#fabd2f',
  brightBlue: '#83a598',
  brightMagenta: '#d3869b',
  brightCyan: '#8ec07c',
  brightWhite: '#ebdbb2',
};

export function createTerminalPane() {
  const container = el('div', { class: 'terminal-pane' });

  // Map of sessionId -> { terminal, fitAddon, element }
  const instances = new Map();
  let currentSessionId = null;
  let outputUnlisten = null;

  async function setupOutputListener() {
    if (outputUnlisten) return;
    outputUnlisten = await api.onTerminalOutput((payload) => {
      const instance = instances.get(payload.session_id);
      if (instance) {
        instance.terminal.write(payload.data);
      }
    });
  }

  async function getOrCreateInstance(sessionId) {
    if (instances.has(sessionId)) return instances.get(sessionId);

    await loadXterm();
    if (!Terminal) return null;

    const wrapper = el('div', { class: 'terminal-pane__instance' });

    const terminal = new Terminal({
      theme: GRUVBOX_THEME,
      fontFamily: '"JetBrains Mono", "Cascadia Code", "Fira Code", monospace',
      fontSize: 13,
      lineHeight: 1.2,
      cursorBlink: true,
      convertEol: true,
      allowProposedApi: true,
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);

    terminal.open(wrapper);

    // Send keystrokes to backend
    terminal.onData((data) => {
      api.writeTerminal(sessionId, data);
    });

    const instance = { terminal, fitAddon, element: wrapper };
    instances.set(sessionId, instance);

    // Fit after a small delay to ensure DOM is laid out
    requestAnimationFrame(() => {
      fitAddon.fit();
      const dims = fitAddon.proposeDimensions();
      if (dims) {
        api.resizeTerminal(sessionId, dims.cols, dims.rows);
      }
    });

    return instance;
  }

  async function switchToSession(sessionId) {
    if (sessionId === currentSessionId) return;
    currentSessionId = sessionId;

    // Hide all
    for (const [, inst] of instances) {
      inst.element.style.display = 'none';
    }

    if (!sessionId) return;

    await setupOutputListener();
    const instance = await getOrCreateInstance(sessionId);
    if (!instance) return;

    if (!instance.element.parentNode) {
      container.appendChild(instance.element);
    }
    instance.element.style.display = 'block';

    requestAnimationFrame(() => {
      instance.fitAddon.fit();
      instance.terminal.focus();
    });
  }

  // React to active session changes
  terminalStore.subscribe('activeSessionId', (sessionId) => {
    switchToSession(sessionId);
  });

  // Clean up instances when sessions are removed
  terminalStore.subscribe('sessions', (sessions) => {
    const sessionIds = new Set(sessions.map(s => s.id));
    for (const [id, inst] of instances) {
      if (!sessionIds.has(id)) {
        inst.terminal.dispose();
        inst.element.remove();
        instances.delete(id);
      }
    }
  });

  // Handle resize of the container
  const resizeObserver = new ResizeObserver(() => {
    if (currentSessionId) {
      const instance = instances.get(currentSessionId);
      if (instance) {
        instance.fitAddon.fit();
        const dims = instance.fitAddon.proposeDimensions();
        if (dims) {
          api.resizeTerminal(currentSessionId, dims.cols, dims.rows);
        }
      }
    }
  });
  resizeObserver.observe(container);

  return container;
}
