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

// [term-diag] TEMP: hang the registry off globalThis so a Vite HMR module
// reload does NOT reset it to empty. Without this, saving this file (or any
// dependency) orphans every live xterm instance and forces a replay-from-ring
// that looks identical to the production history-loss bug — polluting the repro.
// In a production build there's no HMR, so this is a harmless alias. Grep: term-diag.
const instances = (globalThis.__rusticTerminalInstances ??= new Map());

// [term-diag] TEMP: announce module (re)loads so we can tell an HMR reload apart
// from a real dispose in the console timeline.
if (typeof import.meta !== 'undefined' && import.meta.hot) {
  console.warn('[term-diag] terminal-instance.js (re)loaded — instances map size=' + instances.size);
}

// [term-diag] TEMP: console-callable state dump for every live terminal. Run
// `__rusticTermDebug()` in DevTools at the moment the scroll/stale-history bug
// is on screen. Watch `bufferType` (alternate => no scrollback by design) and
// `rows` vs the content you expect. Grep: term-diag.
if (typeof globalThis !== 'undefined') {
  globalThis.__rusticTermDebug = () => {
    console.warn('[term-diag] __rusticTermDebug: ' + instances.size + ' instance(s)');
    for (const inst of instances.values()) {
      try { inst.debug?.('manual'); } catch (e) { console.warn('[term-diag] dump failed', e); }
    }
  };
}

/** Get the persistent instance for a session, creating it on first request. */
export function acquireTerminalInstance(sessionId) {
  let inst = instances.get(sessionId);
  if (!inst) {
    // [term-diag] TEMP: a NEW instance is being created. If this fires for a
    // session that already had history, the old instance was lost (disposed or
    // HMR-wiped) — the stack trace shows who triggered it. Grep: term-diag.
    console.warn('[term-diag] CREATE new terminal instance session=' + sessionId +
      ' (existingIds=[' + [...instances.keys()].join(',') + '])');
    inst = createTerminalInstance(sessionId);
    instances.set(sessionId, inst);
  }
  return inst;
}

/** Tear down and free a terminal instance (called when the PTY is closed). */
export function disposeTerminalInstance(sessionId) {
  const inst = instances.get(sessionId);
  if (!inst) return;
  // [term-diag] TEMP: ANY dispose path (explicit close OR reconcile) lands here.
  // Logs which session + how much scrollback dies + the caller stack. Grep: term-diag.
  try {
    const buf = inst.term?.buffer?.active;
    console.warn('[term-diag] DISPOSE terminal instance session=' + sessionId +
      ' scrollbackLines=' + (buf ? buf.length : '(no term)'));
  } catch (_) {}
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
    if (!liveIds.has(id)) {
      // [term-diag] TEMP: this is the destructive moment — an xterm instance
      // (and its 10k-line scrollback) is being freed because its session id was
      // absent from the latest `list_terminals` snapshot. Log which session, how
      // much history is being thrown away, and the snapshot that triggered it, so
      // we can tell a real shell-exit from a transient/partial listing flap.
      // Remove once the "lost terminal history" repro is understood. Grep: term-diag.
      console.warn('[term-diag] RECONCILE wants to dispose session=' + id +
        ' (liveIds=[' + [...liveIds].join(',') + '] allInstanceIds=[' + [...instances.keys()].join(',') + '])');
      disposeTerminalInstance(id);
    }
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
  let webgl = null; // WebGL renderer addon (null when unavailable / DOM fallback)
  let opened = false;
  let disposed = false;
  let unsubOutput = null;
  let onDataDisposable = null;
  let onScrollDisposable = null;
  let firstRenderDisposable = null;
  let searchResultsDisposable = null;
  let pasteListenerTarget = null;
  let lastCols = 0;
  let lastRows = 0;
  // Escape hatch / A-B test: set localStorage 'rustic.terminal.renderer' to
  // 'dom' to disable the WebGL renderer. The DOM renderer has no glyph texture
  // atlas, so it can't garble scrollback — handy to confirm an atlas issue or as
  // a bulletproof fallback. Anything else (default) keeps WebGL.
  const useWebgl = (() => {
    try {
      return localStorage.getItem('rustic.terminal.renderer') !== 'dom';
    } catch {
      return true;
    }
  })();
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

  // [term-diag] TEMP: dump xterm's live geometry/scroll state. The decisive
  // fields: `bufferType` ('alternate' = the app is on the alt-screen, which has
  // NO scrollback — that alone explains "can't scroll up"); `rows` vs the PTY
  // size (a mismatch explains "zoom out reveals more"); and baseY/length (how
  // much scrollback exists). Call `window.__rusticTermDebug()` in the console at
  // the exact moment the problem is on screen. Grep: term-diag.
  const debugDump = (tag) => {
    try {
      if (!term) {
        console.warn('[term-diag] dump ' + tag + ' session=' + sessionId + ' term=null');
        return;
      }
      const b = term.buffer?.active;
      // Mouse tracking != 'none' means the app is grabbing wheel events, so the
      // terminal buffer won't scroll on wheel — a non-alt-screen cause of
      // "can't scroll up". `term.modes` is xterm's public mode snapshot.
      let modes = '?';
      try { modes = JSON.stringify(term.modes); } catch (_) {}
      // Container vs grid geometry: if containerH is, say, ~120px while rows=82
      // (82 * ~16px ≈ 1300px), the grid is FAR taller than its pane — it never
      // re-fit to the real height, so most of the grid is clipped. parentH is
      // the mount node's height for comparison.
      let containerH = '?', containerW = '?', parentH = '?';
      try {
        const r = container.getBoundingClientRect();
        containerH = Math.round(r.height);
        containerW = Math.round(r.width);
        const pr = container.parentNode && container.parentNode.getBoundingClientRect();
        parentH = pr ? Math.round(pr.height) : 'no-parent';
      } catch (_) {}
      console.warn(
        '[term-diag] dump ' + tag + ' session=' + sessionId +
        ' cols=' + term.cols + ' rows=' + term.rows +
        ' containerH=' + containerH + ' containerW=' + containerW + ' parentH=' + parentH +
        ' bufferType=' + (b && b.type) +
        ' length=' + (b && b.length) +
        ' baseY=' + (b && b.baseY) +
        ' viewportY=' + (b && b.viewportY) +
        ' cursorX=' + (b && b.cursorX) +
        ' cursorY=' + (b && b.cursorY) +
        ' modes=' + modes
      );
    } catch (e) {
      console.warn('[term-diag] dump error', e);
    }
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
        if (disposed || !useWebgl) return;
        try {
          const addon = new WebglAddon();
          addon.onContextLoss(() => {
            // GPU context lost (driver crash / GPU mode switch, or the browser
            // evicting an old WebGL context) — dispose so xterm falls back to the
            // DOM renderer instead of a dead canvas, and drop our reference so the
            // scroll/repaint handlers stop poking a disposed addon.
            try { addon.dispose(); } catch (_) {}
            if (webgl === addon) webgl = null;
          });
          term.loadAddon(addon);
          webgl = addon;
        } catch (err) {
          webgl = null;
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

      // Rehydrate history from the headless emulator's RESOLVED grid (clean,
      // de-duplicated ANSI) rather than the raw ConPTY byte ring. The raw ring
      // captures every repaint/resize frame ConPTY emits, which xterm commits to
      // scrollback as duplicate lines; the emulator collapses those to the final
      // grid state, so scrollback rehydrates exactly once. We write the snapshot
      // first, then subscribe to the live stream, so history and live output
      // stay in order.
      useTerminal.getState().readTerminalScrollback(sessionId).then((snapshot) => {
        if (disposed || !term) return;
        // [term-diag] TEMP: how big was the rehydrated history?
        console.warn('[term-diag] REPLAY session=' + sessionId +
          ' snapshotBytes=' + (snapshot ? snapshot.length : 0) +
          ' snapshotLines=' + (snapshot ? snapshot.split('\n').length : 0));
        if (snapshot) {
          term.write(snapshot, () => debugDump('after-replay'));
        }
        unsubOutput = useTerminal.getState().subscribeOutput(sessionId, writeData);
      });

      onDataDisposable = term.onData((d) => useTerminal.getState().writeTerminal(sessionId, d));

      // Scrolling into history re-renders those rows against the WebGL glyph
      // texture atlas. Over a long session — and especially after the canvas was
      // hidden on a tab switch and the atlas went stale — those cached glyph
      // slots can draw the wrong characters ("scrambled scrollback") or, worse,
      // render the newly-revealed rows BLANK. Clearing the atlas alone isn't
      // enough: xterm won't repaint rows it considers unchanged, so the cleared
      // slots stay empty until something else dirties them. We must follow the
      // clear with an explicit `refresh()` of the viewport to force those rows to
      // re-rasterize against the fresh atlas. Throttled so a fast scroll gesture
      // doesn't thrash the GPU; no-op under the DOM renderer.
      let lastAtlasClear = 0;
      onScrollDisposable = term.onScroll(() => {
        if (!webgl || !term) return;
        // Only when scrolled UP into history. At the live bottom the atlas is
        // fine, and clearing it there would thrash the GPU on every auto-scroll
        // during streaming output — re-introducing the flicker WebGL prevents.
        const buf = term.buffer?.active;
        if (!buf || buf.viewportY >= buf.baseY) return;
        const now = Date.now();
        if (now - lastAtlasClear < 80) return;
        lastAtlasClear = now;
        try { webgl.clearTextureAtlas?.(); } catch (_) {}
        // Repaint the visible rows so the just-cleared atlas is repopulated —
        // without this the scrolled-into rows can paint blank.
        try { term.refresh(0, Math.max(0, term.rows - 1)); } catch (_) {}
      });

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

  // Force a clean repaint of the visible screen, rebuilding the WebGL glyph atlas
  // first. Called when a terminal becomes visible again after a tab switch (its
  // canvas was display:none), so a stale atlas can't garble what's drawn.
  const repaint = () => {
    if (!term) return;
    try { webgl?.clearTextureAtlas?.(); } catch (_) {}
    try { term.refresh(0, Math.max(0, term.rows - 1)); } catch (_) {}
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
    onScrollDisposable?.dispose();
    try { unsubOutput?.(); } catch (_) {}
    if (pasteListenerTarget) {
      pasteListenerTarget.removeEventListener('paste', onTextareaPaste, true);
      pasteListenerTarget = null;
    }
    const t = term;
    term = null;
    fit = null;
    search = null;
    webgl = null;
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
    repaint,
    debug: debugDump, // [term-diag] TEMP
    setOnOpenSearch(cb) { onOpenSearch = cb; },
    subscribeSearchResults(cb) {
      searchResultSubs.add(cb);
      cb(lastSearchResults);
      return () => searchResultSubs.delete(cb);
    },
    dispose,
  };
}
