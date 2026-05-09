import { el } from '../../utils/dom.js';
import { terminalStore, closeTerminal as closeTerminalSession, createTerminal as createTerminalSession, splitTerminal as splitTerminalSession } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';
import { showContextMenu } from '../dropdown-menu.js';
import { getXtermTheme } from '../../lib/theme.js';

// We'll dynamically import xterm to handle the case where it might not be available
let Terminal, FitAddon, WebglAddon;

// Tracks the most recent xterm load failure so we can surface it inside the
// pane. Vite's optimized-dep cache can go stale ("504 Outdated Optimize Dep")
// and silently swallow the dynamic import — without a visible error the pane
// just renders an empty void with a tab label, which looks identical to a
// renderer-dimensions bug. Capturing the error lets us tell the user exactly
// what happened and how to fix it.
let xtermLoadError = null;

async function loadXterm() {
  if (Terminal) return;
  try {
    const xtermMod = await import('xterm');
    const fitMod = await import('@xterm/addon-fit');
    Terminal = xtermMod.Terminal;
    FitAddon = fitMod.FitAddon;

    // WebGL addon: meaningfully faster than the default canvas renderer for
    // high-throughput output (cat large file, npm install spam, log floods).
    // Optional — falls back silently if WebGL isn't available (headless / GPU
    // blacklist / Wayland edge cases).
    try {
      const webglMod = await import('@xterm/addon-webgl');
      WebglAddon = webglMod.WebglAddon;
    } catch {
      WebglAddon = null;
    }

    await import('xterm/css/xterm.css');
    xtermLoadError = null;
  } catch (e) {
    console.error('Failed to load xterm:', e);
    xtermLoadError = e;
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
  // Buffer output that arrives before its xterm instance has been opened.
  // Without this the very first chunk from a freshly-spawned shell — which
  // includes the initial prompt — is silently dropped because:
  //   1. createTerminal() returns a session id
  //   2. the pty reader thread on the backend immediately emits "terminal-output"
  //   3. the user-facing instance hasn't been built yet (renderSplit runs next)
  //   4. terminal.write would target a not-yet-existing xterm
  // We stash incoming data per-session and replay it on getOrCreateInstance.
  const pendingOutput = new Map(); // sessionId -> string[]
  let outputUnlisten = null;

  async function setupOutputListener() {
    if (outputUnlisten) return;
    outputUnlisten = await api.onTerminalOutput((payload) => {
      const instance = instances.get(payload.session_id);
      if (instance && instance.opened) {
        instance.terminal.write(payload.data);
      } else {
        // xterm not ready yet — queue for replay once the instance opens.
        const queue = pendingOutput.get(payload.session_id) || [];
        queue.push(payload.data);
        pendingOutput.set(payload.session_id, queue);
      }
    });
  }

  // Wire the listener up eagerly at pane construction. Don't wait for
  // renderSplit — by the time the user opens a fresh terminal the pty's
  // reader thread is already racing to emit the shell prompt, so the
  // listener has to exist before createTerminal() resolves.
  setupOutputListener();

  async function getOrCreateInstance(sessionId) {
    if (instances.has(sessionId)) return instances.get(sessionId);

    await loadXterm();
    if (!Terminal) {
      // Render an inline error inside the pane so the user sees what's
      // wrong instead of just a blank black box. Most common cause in dev
      // is a stale Vite optimized-dep cache (504 Outdated Optimize Dep) —
      // the message points at the fix.
      container.innerHTML = '';
      const msg = xtermLoadError && xtermLoadError.message ? xtermLoadError.message : 'Unknown error';
      const errBox = el('div', {
        style: 'padding:16px;color:var(--fg2);font-family:var(--font-family-mono);font-size:12px;white-space:pre-wrap;line-height:1.5;',
      });
      errBox.textContent =
        `xterm failed to load — terminal cannot render.\n\n` +
        `${msg}\n\n` +
        `Common fix (Vite stale dep cache): stop the dev server, delete\n` +
        `node_modules/.vite, then restart with "npm run tauri dev".`;
      container.appendChild(errBox);
      return null;
    }

    const wrapper = el('div', { class: 'terminal-pane__instance' });

    const terminal = new Terminal({
      theme: getXtermTheme() || GRUVBOX_THEME,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-family-terminal').trim() || '"JetBrains Mono", "Cascadia Code", "Fira Code", monospace',
      fontSize: 13,
      lineHeight: 1.2,
      cursorBlink: true,
      convertEol: true,
      allowProposedApi: true,
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    // WebGL addon must be loaded AFTER terminal.open() (it needs a DOM
    // canvas). See attachWebglAddon() — called from the open path below.

    // Ctrl+C semantics matching VS Code / gnome-terminal: if there's an
    // active selection, Ctrl+C copies it and swallows the event so the
    // shell does NOT receive SIGINT. With no selection, xterm handles it
    // normally and sends ^C to the pty. Ctrl+Shift+C is also treated as
    // copy (standard terminal convention, since Ctrl+C without selection
    // is ambiguous with interrupt).
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown') return true;
      const isCtrlV = event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey && (event.key === 'v' || event.key === 'V');
      if (isCtrlV) {
        navigator.clipboard.readText().then(text => {
          if (text) api.writeTerminal(sessionId, text);
        }).catch(() => {});
        return false;
      }

      const isCtrlC = event.ctrlKey && !event.altKey && !event.metaKey && (event.key === 'c' || event.key === 'C');
      if (!isCtrlC) return true;

      const selection = terminal.getSelection();
      const hasSelection = selection && selection.length > 0;

      // Ctrl+Shift+C → always copy (if anything is selected)
      if (event.shiftKey) {
        if (hasSelection) {
          navigator.clipboard.writeText(selection).catch(() => {});
          terminal.clearSelection();
        }
        return false;
      }

      // Plain Ctrl+C: only copy when there's a selection. Otherwise fall
      // through so xterm sends ^C to the shell (interrupt).
      if (hasSelection) {
        navigator.clipboard.writeText(selection).catch(() => {});
        terminal.clearSelection();
        return false;
      }
      return true;
    });

    // NOTE: terminal.open(wrapper) is deferred to renderSplit, called only after
    // the wrapper is in the DOM so xterm can measure real dimensions immediately.

    // Right-click context menu for the terminal
    wrapper.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      e.stopPropagation();
      showContextMenu([
        {
          label: 'Paste',
          action: () => {
            navigator.clipboard.readText().then(text => {
              if (text) api.writeTerminal(sessionId, text);
            }).catch(() => {});
          },
        },
        { separator: true },
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
      if (dims && dims.cols > 0 && dims.rows > 0) {
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

      // Highlight active terminal
      instance.element.classList.toggle('terminal-pane__instance--active', sessionId === activeId);

      // Open xterm only after the browser has actually laid out the wrapper.
      // If we call .open() while the bottom panel is still mid-reveal (panel
      // height transitioning from 0 → 200, or display switching from none to
      // flex), xterm's renderer captures bogus 0×0 dimensions and locks in a
      // broken canvas — the terminal looks like a hollow shell with no
      // cursor or output ever appearing. Waiting one frame guarantees the
      // grid row + flex children have non-zero size before xterm measures.
      if (!instance.opened) {
        instance.opened = true;
        const openAndFit = () => {
          // Bail if the wrapper still has no size (e.g. user toggled the
          // panel back closed before the frame fired). Try again next frame.
          const rect = instance.element.getBoundingClientRect();
          if (rect.width < 1 || rect.height < 1) {
            requestAnimationFrame(openAndFit);
            return;
          }
          instance.terminal.open(instance.element);
          patchXtermZoomFix(instance.terminal);
          // WebGL renderer requires the canvas to be in the DOM, so attach
          // it now (just after open). Falls back silently to canvas if WebGL
          // isn't available.
          if (WebglAddon) {
            try {
              const webgl = new WebglAddon();
              webgl.onContextLoss(() => webgl.dispose());
              instance.terminal.loadAddon(webgl);
            } catch (e) {
              console.warn('[terminal] WebGL renderer unavailable, using canvas:', e);
            }
          }
          // Replay any pty output that arrived before xterm was ready (the
          // shell prompt is the typical victim of this race — the user would
          // otherwise see an empty terminal until they pressed Enter).
          const queued = pendingOutput.get(sessionId);
          if (queued && queued.length > 0) {
            for (const chunk of queued) instance.terminal.write(chunk);
            pendingOutput.delete(sessionId);
          }
          fitInstance(sessionId);
          // Final fit after CSS transitions (panel slide-in) settle.
          setTimeout(() => fitInstance(sessionId), 150);
          if (sessionId === terminalStore.getState('activeSessionId')) {
            instance.terminal.focus();
          }
        };
        requestAnimationFrame(openAndFit);
      } else {
        // Already-opened instance just becoming visible again — refit.
        requestAnimationFrame(() => {
          fitInstance(sessionId);
          setTimeout(() => fitInstance(sessionId), 150);
        });
        if (sessionId === activeId) {
          requestAnimationFrame(() => instance.terminal.focus());
        }
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

  window.addEventListener('rustic:theme-changed', () => {
    const xtermTheme = getXtermTheme();
    if (!xtermTheme) return;
    for (const instance of instances.values()) {
      if (!instance.opened) continue;
      try {
        instance.terminal.options.theme = xtermTheme;
      } catch { /* WebGL context may be unavailable on detached canvas */ }
    }
  });

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
          if (dims && dims.cols > 0 && dims.rows > 0) api.resizeTerminal(id, dims.cols, dims.rows);
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
