import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { SearchAddon } from '@xterm/addon-search';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { useTerminal } from '@/state/terminal';
import {
  handleTerminalPaste,
  pasteViaClipboardApi,
  copyTerminalSelection,
} from '@/lib/terminal-clipboard';

// ---------------------------------------------------------------------------
// Persistent terminal instances.
//
// The xterm.js instance (and crucially its scrollback buffer) is the live,
// in-memory history of a terminal. React would otherwise destroy it whenever
// the <TerminalPane> unmounts — which happens on EVERY structural layout change
// (fullscreen toggle, chat-dock toggle, …) because those relocate the panel
// subtree in the React tree. Destroying it wipes the visible history even
// though the backend PTY keeps running, so coming back shows an empty screen.
//
// Instead we own the instance + its DOM element outside React, keyed by session
// id. <TerminalPane> merely *reparents* the persistent element into its mount
// node on mount and detaches it on unmount — the xterm instance, its buffer,
// and its 10k-line scrollback survive across any number of remounts. The
// instance is disposed only when the terminal is actually closed (see the store
// calling disposeTerminalInstance / reconcileTerminalInstances), at which point
// its memory is freed immediately.
// ---------------------------------------------------------------------------

// xterm's renderer requires monospace, so we don't expose terminal font
// customization in appearance settings. The terminal always uses Victor Mono,
// the IDE's bundled default code font (see globals.css @font-face). Bundling it
// as WOFF2 guarantees the same cell metrics + glyph coverage on every machine —
// the previous stack relied on a system-installed Cascadia Mono and silently
// fell back to Consolas (different metrics) when it was absent, which is what
// made block/Unicode glyphs render misaligned. The tail is fallback-only.
const TERMINAL_FONT_FAMILY = "'Victor Mono', 'Cascadia Mono', Consolas, monospace";

// VS Code's terminal palette. Without these xterm.js falls back to its own
// defaults, which are saturated near-pure ANSI colors (#ff0000 red, etc.).
const TERMINAL_PALETTE = {
  black:         '#000000',
  red:           '#cd3131',
  green:         '#0dbc79',
  yellow:        '#e5e510',
  blue:          '#2472c8',
  magenta:       '#bc3fbc',
  cyan:          '#11a8cd',
  white:         '#e5e5e5',
  brightBlack:   '#666666',
  brightRed:     '#f14c4c',
  brightGreen:   '#23d18b',
  brightYellow:  '#f5f543',
  brightBlue:    '#3b8eea',
  brightMagenta: '#d670d6',
  brightCyan:    '#29b8db',
  brightWhite:   '#e5e5e5',
};

const instances = new Map();

/** Get the persistent instance for a session, creating it on first request. */
export function acquireTerminalInstance(sessionId) {
  let inst = instances.get(sessionId);
  if (!inst) {
    inst = createTerminalInstance(sessionId);
    instances.set(sessionId, inst);
  }
  return inst;
}

/** Tear down and free a terminal instance (called when the PTY is closed). */
export function disposeTerminalInstance(sessionId) {
  const inst = instances.get(sessionId);
  if (!inst) return;
  instances.delete(sessionId);
  inst.dispose();
}

/**
 * Dispose any instances whose session id is no longer alive. Called after the
 * session list refreshes so a terminal that died on the backend (process exit,
 * crash) doesn't leak its xterm instance + buffer.
 */
export function reconcileTerminalInstances(liveIds) {
  for (const id of [...instances.keys()]) {
    if (!liveIds.has(id)) disposeTerminalInstance(id);
  }
}

function createTerminalInstance(sessionId) {
  // The persistent host element. xterm opens into this; the React component
  // appends it to its mount node and removes it on unmount. Reparenting a DOM
  // node preserves its descendants (xterm's canvas), and the buffer lives in
  // the JS instance regardless, so nothing is lost.
  const container = document.createElement('div');
  container.style.height = '100%';
  container.style.width = '100%';
  container.style.overflow = 'hidden';

  let term = null;
  let fit = null;
  let search = null;
  let opened = false;
  let disposed = false;
  let unsubOutput = null;
  let onDataDisposable = null;
  let firstRenderDisposable = null;
  let searchResultsDisposable = null;
  let pasteListenerTarget = null;
  let lastCols = 0;
  let lastRows = 0;
  // Callback set by the mounted component so Ctrl+Shift+F can open its overlay.
  let onOpenSearch = null;
  const searchResultSubs = new Set();
  let lastSearchResults = { index: -1, count: 0 };

  // Set by our paste-event listener whenever it handles a paste, so the keydown
  // handler can tell whether the browser delivered a paste event.
  let lastPasteEventAt = 0;
  const onTextareaPaste = (e) => {
    // Capture-phase listener: runs BEFORE xterm.js's own paste listener. We
    // stopImmediatePropagation to keep xterm from also pasting (double-write).
    e.preventDefault();
    e.stopImmediatePropagation();
    lastPasteEventAt = Date.now();
    if (!term) return;
    const session = useTerminal.getState().sessions.find((s) => s.id === sessionId);
    handleTerminalPaste(term, session?.cwd, e.clipboardData);
  };

  const emitSearchResults = (r) => {
    lastSearchResults = r;
    searchResultSubs.forEach((cb) => {
      try { cb(r); } catch (_) {}
    });
  };

  // Create the xterm instance and wire all addons/handlers. Deferred until the
  // host element actually has a non-zero size (it's hidden until its tab/pane
  // becomes visible) — xterm measures cell metrics at open() time, so opening
  // a 0×0 element sizes the grid wrong.
  const openTerminal = () => {
    if (opened || disposed) return;
    const { width, height } = container.getBoundingClientRect();
    if (width === 0 || height === 0) return; // still hidden — wait for resize
    opened = true;

    term = new Terminal({
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: 13,
      // 1.0 matches what most CLI logos (block-character pixel art) are designed
      // against. The default 1.2 stretches cells vertically and distorts them.
      lineHeight: 1.0,
      // Steady, non-blinking cursor. TUIs like Claude Code (Ink) park the
      // cursor on their animated status rows and rely on the host terminal
      // hiding it during redraws; our ConPTY-backed xterm keeps it visible, so
      // a blinking cursor looked like a stray cursor "stuck blinking" on the
      // animation row. A steady cursor removes that distraction.
      cursorBlink: false,
      // Hide the cursor entirely when this terminal pane isn't the focused
      // element (e.g. while watching an agent/Claude Code work with focus in
      // the chat box) — so no cursor artifact shows on a repainting TUI row.
      cursorInactiveStyle: 'none',
      theme: {
        background:          '#0a0a0a',
        foreground:          '#e5e5e5',
        cursor:              '#e5e5e5',
        selectionBackground: '#264f78',
        ...TERMINAL_PALETTE,
      },
      // Retain a deep scrollback so history is browsable for the life of the
      // terminal. Freed when the instance is disposed (terminal closed).
      scrollback: 10000,
      // Tell xterm.js the host PTY is Windows ConPTY so it applies the
      // ConPTY-specific cursor / line-wrap workarounds; without it, TUIs that
      // redraw frames appear to shift during streaming.
      ...(navigator.userAgent.includes('Windows')
        ? { windowsPty: { backend: 'conpty' } }
        : {}),
      // ConPTY already emits proper \r\n; converting again inserts spurious
      // blank lines and forces TUIs to redraw at shifted positions.
      convertEol: false,
      smoothScrollDuration: 0,
      allowProposedApi: true,
      rescaleOverlappingGlyphs: true,
      minimumContrastRatio: 4.5,
    });

    fit = new FitAddon();
    term.loadAddon(fit);

    // Wait for the terminal font to actually load before opening — xterm
    // measures the font's cell width at open() time; with the @font-face still
    // loading we'd get the fallback's metrics and misaligned block glyphs.
    const fontReady = (typeof document !== 'undefined' && document.fonts?.ready)
      ? document.fonts.ready
      : Promise.resolve();

    fontReady.then(() => {
      if (disposed) return;
      try {
        term.open(container);
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error('[terminal] term.open() failed', e);
        return;
      }

      // Unicode v11 width tables (xterm defaults to v6, which sizes many modern
      // glyphs as 1 cell when they occupy 2). Must be set before the first fit.
      try {
        const unicode11 = new Unicode11Addon();
        term.loadAddon(unicode11);
        term.unicode.activeVersion = '11';
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('[terminal] unicode11 addon unavailable:', err);
      }

      // Clickable URLs, opened via the OS default browser (Tauri shell).
      try {
        const webLinks = new WebLinksAddon((event, uri) => {
          openUrl(uri).catch((err) => {
            // eslint-disable-next-line no-console
            console.warn('[terminal] failed to open link', uri, err);
          });
        });
        term.loadAddon(webLinks);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('[terminal] web-links addon unavailable:', err);
      }

      // Find-in-terminal (Ctrl+Shift+F). Searches xterm's own buffer.
      try {
        search = new SearchAddon();
        term.loadAddon(search);
        searchResultsDisposable = search.onDidChangeResults((e) => {
          emitSearchResults({ index: e.resultIndex, count: e.resultCount });
        });
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('[terminal] search addon unavailable:', err);
      }

      // WebGL renderer — GPU-accelerated, batches paints to kill the flicker
      // the DOM renderer shows under high-frequency TUI redraws. It MUST be
      // loaded only after the first frame has rendered: the addon's
      // `coreBrowserService.mainDocument` (used to create its canvas) isn't
      // wired up until the renderer is live, so loading it synchronously or in
      // a microtask right after open() throws "Cannot read properties of
      // undefined (reading 'createElement')" and silently drops every terminal
      // to the slow DOM renderer. A genuine WebGL2 absence (some WebView2 GPU
      // configs) still falls back to DOM cleanly — that warning is informational.
      const loadWebgl = () => {
        if (disposed) return;
        try {
          const webgl = new WebglAddon();
          webgl.onContextLoss(() => {
            // GPU context lost (driver crash / GPU mode switch) — dispose so
            // xterm falls back to the DOM renderer instead of a dead canvas.
            try { webgl.dispose(); } catch (_) {}
          });
          term.loadAddon(webgl);
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn('[terminal] WebGL renderer unavailable, using DOM fallback:', err);
        }
      };

      // Don't fit synchronously — the renderer initialises asynchronously and
      // fitting before it's live triggers the "dimensions undefined" crash.
      // Wait for the first rendered frame, then fit and upgrade to WebGL.
      firstRenderDisposable = term.onRender(() => {
        firstRenderDisposable?.dispose();
        firstRenderDisposable = null;
        try { fit?.fit(); } catch (_) {}
        if (term?.cols > 0 && term?.rows > 0) {
          useTerminal.getState().resizeTerminal(sessionId, term.cols, term.rows);
        }
        loadWebgl();
      });

      // Suppress xterm's translation of Ctrl+V / Ctrl+C into raw ^V / ^C, and
      // intercept Ctrl+Shift+F for find. Ctrl+C still falls through as ^C when
      // there's no selection so SIGINT keeps working.
      term.attachCustomKeyEventHandler((e) => {
        if (e.type !== 'keydown') return true;
        const mod = e.ctrlKey || e.metaKey;
        if (!mod || e.altKey) return true;
        const k = e.key.toLowerCase();

        if (k === 'f' && e.shiftKey) {
          onOpenSearch?.();
          return false;
        }

        if (k === 'v') {
          // Primary paste path is the capture-phase paste listener (it has sync
          // access to image MIME types). Fall back to navigator.clipboard only
          // if no paste event arrives within ~80ms (a WebView2 focus quirk).
          const startedAt = Date.now();
          setTimeout(() => {
            if (lastPasteEventAt >= startedAt) return;
            const session = useTerminal.getState().sessions.find((s) => s.id === sessionId);
            pasteViaClipboardApi(term, session?.cwd);
          }, 80);
          return false;
        }

        if (k === 'c') {
          if (e.shiftKey) {
            copyTerminalSelection(term);
            return false;
          }
          if (term.hasSelection()) {
            copyTerminalSelection(term);
            term.clearSelection();
            return false;
          }
          return true;
        }

        return true;
      });

      const writeData = (data) => {
        if (typeof data === 'string')      term.write(data);
        else if (data instanceof Uint8Array) term.write(data);
        else if (Array.isArray(data))        term.write(new Uint8Array(data));
      };

      // Replay any output the backend buffered BEFORE this xterm mounted, then
      // attach to the live stream. Without this an agent-spawned terminal —
      // whose commands run before the user ever opens its pane — renders blank,
      // because the live `terminal-output` stream only carries bytes from the
      // moment of subscription. We write the snapshot first, then subscribe, so
      // history and live output stay in order (a finished/idle agent terminal,
      // the common case here, has no concurrent output to gap).
      useTerminal.getState().readTerminalBuffer(sessionId).then((snapshot) => {
        if (disposed || !term) return;
        if (snapshot) term.write(snapshot);
        unsubOutput = useTerminal.getState().subscribeOutput(sessionId, writeData);
      });

      onDataDisposable = term.onData((d) => useTerminal.getState().writeTerminal(sessionId, d));

      // Install our paste handler on xterm's hidden helper textarea in capture
      // phase (does the paste + stopImmediatePropagation's xterm's listener).
      const helperTextarea = container.querySelector('textarea');
      if (helperTextarea) {
        pasteListenerTarget = helperTextarea;
        helperTextarea.addEventListener('paste', onTextareaPaste, true);
      }

      if (term.cols > 0 && term.rows > 0) {
        useTerminal.getState().resizeTerminal(sessionId, term.cols, term.rows);
      }
    });
  };

  const refit = () => {
    if (!term || !fit) return;
    const { width, height } = container.getBoundingClientRect();
    if (width === 0 || height === 0) return;
    try {
      fit.fit();
      if (term.cols > 0 && term.rows > 0 &&
          (term.cols !== lastCols || term.rows !== lastRows)) {
        lastCols = term.cols;
        lastRows = term.rows;
        useTerminal.getState().resizeTerminal(sessionId, term.cols, term.rows);
      }
    } catch (_) {}
  };

  // ResizeObserver serves double duty: the first non-zero observation opens the
  // terminal; subsequent ones re-fit. Reparenting (mount → unmount → mount) and
  // tile resizes both surface here. We skip the resize IPC when cols/rows are
  // unchanged so sub-pixel container changes don't spam the PTY.
  const ro = new ResizeObserver(() => {
    if (!opened) {
      openTerminal();
      return;
    }
    refit();
  });
  ro.observe(container);

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    try { ro.disconnect(); } catch (_) {}
    firstRenderDisposable?.dispose();
    searchResultsDisposable?.dispose();
    onDataDisposable?.dispose();
    try { unsubOutput?.(); } catch (_) {}
    if (pasteListenerTarget) {
      pasteListenerTarget.removeEventListener('paste', onTextareaPaste, true);
      pasteListenerTarget = null;
    }
    const t = term;
    term = null;
    fit = null;
    search = null;
    searchResultSubs.clear();
    if (t) {
      try { t.dispose(); } catch (_) {}
    }
    try { container.remove(); } catch (_) {}
  };

  return {
    sessionId,
    container,
    get term() { return term; },
    get search() { return search; },
    // Called by the mounted component once the element is attached: kick the
    // open (no-op if already open) and re-fit to the current size.
    attach() {
      openTerminal();
      refit();
    },
    refit,
    setOnOpenSearch(cb) { onOpenSearch = cb; },
    subscribeSearchResults(cb) {
      searchResultSubs.add(cb);
      cb(lastSearchResults);
      return () => searchResultSubs.delete(cb);
    },
    dispose,
  };
}
