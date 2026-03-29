import { el } from '../../utils/dom.js';
import { terminalStore, closeTerminal as closeTerminalSession, createTerminal as createTerminalSession, splitTerminal as splitTerminalSession } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';
import { showContextMenu } from '../dropdown-menu.js';

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
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-family-terminal').trim() || '"JetBrains Mono", "Cascadia Code", "Fira Code", monospace',
      fontSize: 13,
      lineHeight: 1.2,
      cursorBlink: true,
      convertEol: true,
      allowProposedApi: true,
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);

    // NOTE: terminal.open(wrapper) is deferred to renderSplit, called only after
    // the wrapper is in the DOM so xterm can measure real dimensions immediately.

    // Right-click context menu for the terminal
    wrapper.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      e.stopPropagation();
      showContextMenu([
        {
          label: 'Clear Terminal',
          action: () => terminal.clear(),
        },
        { separator: true },
        {
          label: 'Kill Terminal',
          action: () => closeTerminalSession(sessionId),
        },
        { separator: true },
        {
          label: 'New Terminal',
          action: () => createTerminalSession(null),
        },
        {
          label: 'Split Terminal',
          action: () => splitTerminalSession(null),
        },
      ], e.clientX, e.clientY);
    }, true);

    // Send keystrokes to backend
    terminal.onData((data) => {
      api.writeTerminal(sessionId, data);
    });

    // Clicking this terminal focuses it
    wrapper.addEventListener('mousedown', () => {
      terminalStore.setState({ activeSessionId: sessionId });
    });

    const instance = { terminal, fitAddon, element: wrapper, opened: false };
    instances.set(sessionId, instance);

    return instance;
  }

  function fitInstance(sessionId) {
    const instance = instances.get(sessionId);
    if (!instance) return;
    try {
      instance.fitAddon.fit();
      const dims = instance.fitAddon.proposeDimensions();
      if (dims) {
        api.resizeTerminal(sessionId, dims.cols, dims.rows);
      }
    } catch { /* element may not be visible yet */ }
  }

  /** Render split sessions side-by-side */
  async function renderSplit() {
    const splitIds = terminalStore.getState('splitSessionIds');
    const activeId = terminalStore.getState('activeSessionId');

    await setupOutputListener();

    // Hide all instances first
    for (const [, inst] of instances) {
      inst.element.style.display = 'none';
    }

    // Clear container and rebuild with split layout
    container.innerHTML = '';

    if (splitIds.length === 0) return;

    for (let i = 0; i < splitIds.length; i++) {
      const sessionId = splitIds[i];

      // Add resize handle between terminals (not before first)
      if (i > 0) {
        container.appendChild(createTerminalResizeHandle());
      }

      const instance = await getOrCreateInstance(sessionId);
      if (!instance) continue;

      if (!instance.element.parentNode || instance.element.parentNode !== container) {
        container.appendChild(instance.element);
      }
      instance.element.style.display = 'flex';
      instance.element.style.flex = '1';
      instance.element.style.minWidth = '80px';

      // Open xterm now that the wrapper is in the DOM so it can measure dimensions.
      // Only do this once per instance.
      if (!instance.opened) {
        instance.opened = true;
        instance.terminal.open(instance.element);
        patchXtermZoomFix(instance.terminal);
      }

      // Highlight active terminal
      instance.element.classList.toggle('terminal-pane__instance--active', sessionId === activeId);

      // Fit immediately, then retry after layout settles (CSS transitions, panel reveal)
      requestAnimationFrame(() => {
        fitInstance(sessionId);
        setTimeout(() => fitInstance(sessionId), 150);
      });

      // Focus active terminal after it's been opened and in the DOM
      if (sessionId === activeId) {
        requestAnimationFrame(() => instance.terminal.focus());
      }
    }
  }

  // React to split layout and active session changes
  terminalStore.subscribe('splitSessionIds', renderSplit);
  terminalStore.subscribe('activeSessionId', () => {
    const activeId = terminalStore.getState('activeSessionId');
    for (const [id, inst] of instances) {
      inst.element.classList.toggle('terminal-pane__instance--active', id === activeId);
    }
    // Focus the active terminal
    const activeInst = instances.get(activeId);
    if (activeInst) activeInst.terminal.focus();
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

  // Handle clear-terminal events dispatched from terminal tabs right-click menu
  window.addEventListener('rustic:clear-terminal', (e) => {
    const instance = instances.get(e.detail.sessionId);
    if (instance) instance.terminal.clear();
  });

  // Handle resize of the container — refit all visible terminals
  const resizeObserver = new ResizeObserver(() => {
    const splitIds = terminalStore.getState('splitSessionIds');
    for (const sessionId of splitIds) {
      fitInstance(sessionId);
    }
  });
  resizeObserver.observe(container);

  return container;
}

/** Resize handle between terminal split panes */
function createTerminalResizeHandle() {
  const handle = el('div', { class: 'terminal-split-handle' });

  let startX = 0;
  let leftEl = null;
  let rightEl = null;
  let totalWidth = 0;
  let leftStart = 0;

  handle.addEventListener('mousedown', (e) => {
    e.preventDefault();
    startX = e.clientX;
    leftEl = handle.previousElementSibling;
    rightEl = handle.nextElementSibling;
    if (!leftEl || !rightEl) return;

    totalWidth = leftEl.offsetWidth + rightEl.offsetWidth;
    leftStart = leftEl.offsetWidth;
    handle.classList.add('active');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    function onMove(e) {
      const delta = e.clientX - startX;
      const newLeft = Math.max(80, Math.min(totalWidth - 80, leftStart + delta));
      const newRight = totalWidth - newLeft;
      leftEl.style.flex = `0 0 ${newLeft}px`;
      rightEl.style.flex = `0 0 ${newRight}px`;
    }

    function onUp() {
      handle.classList.remove('active');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      const leftW = leftEl.offsetWidth;
      const rightW = rightEl.offsetWidth;
      const total = leftW + rightW;
      leftEl.style.flex = `${leftW / total}`;
      rightEl.style.flex = `${rightW / total}`;
      // Refit terminals after resize
      const splitIds = terminalStore.getState('splitSessionIds');
      for (const id of splitIds) {
        const inst = instances.get(id);
        if (inst) {
          inst.fitAddon.fit();
          const dims = inst.fitAddon.proposeDimensions();
          if (dims) api.resizeTerminal(id, dims.cols, dims.rows);
        }
      }
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    }

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  });

  return handle;
}

/**
 * Patch xterm.js's internal MouseService to correct mouse coordinates for
 * the CSS zoom applied to #app. xterm.js calculates:
 *   column = (event.clientX - rect.left) / cellWidth
 * where (event.clientX - rect.left) is viewport-space (zoomed) but cellWidth
 * is measured via offsetWidth in CSS-space. We pre-correct the event so the
 * offset reaching xterm's division is already in CSS-space.
 */
function patchXtermZoomFix(terminal) {
  const core = terminal._core;
  if (!core) return;
  const mouseService = core._mouseService;
  if (!mouseService) return;

  function correctedEvent(event, element) {
    const zoom = parseFloat(document.getElementById('app')?.style.zoom) || 1;
    if (zoom === 1) return event;
    const rect = element.getBoundingClientRect();
    return {
      clientX: (event.clientX - rect.left) / zoom + rect.left,
      clientY: (event.clientY - rect.top) / zoom + rect.top,
    };
  }

  const origGetCoords = mouseService.getCoords.bind(mouseService);
  mouseService.getCoords = function(event, element, colCount, rowCount, isSelection) {
    return origGetCoords(correctedEvent(event, element), element, colCount, rowCount, isSelection);
  };

  if (typeof mouseService.getMouseReportCoords === 'function') {
    const origGetMRC = mouseService.getMouseReportCoords.bind(mouseService);
    mouseService.getMouseReportCoords = function(event, element) {
      return origGetMRC(correctedEvent(event, element), element);
    };
  }
}
