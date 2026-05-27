import React, { useEffect, useRef } from 'react';
import { Terminal } from 'xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import 'xterm/css/xterm.css';
import { useTerminal } from '@/state/terminal';
import {
  handleTerminalPaste,
  pasteViaClipboardApi,
  copyTerminalSelection,
} from '@/lib/terminal-clipboard';

// xterm's renderer requires monospace, so we don't expose terminal font
// customization in appearance settings. The terminal always uses this stack.
// Cascadia Mono is preferred — it ships with Windows Terminal and renders
// the Unicode block characters (▀▄█) that TUI logos use at the same cell
// metrics most CLI tools assume. Consolas/JetBrains Mono are fallbacks if
// it isn't installed.
const TERMINAL_FONT_FAMILY = '"Cascadia Mono", "Cascadia Code", Consolas, "JetBrains Mono", monospace';

// VS Code's terminal palette. Without these xterm.js falls back to its own
// defaults, which are saturated near-pure ANSI colors (#ff0000 red, etc.).
// That's why "red" looked neon in the Rustic terminal vs muted in VS Code.
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

export function TerminalPane({ sessionId, active }) {
  const containerRef  = useRef(null);
  const termRef       = useRef(null);
  const fitRef        = useRef(null);
  const readyRef      = useRef(false); // true once term.open() has succeeded

  const subscribeOutput = useTerminal((s) => s.subscribeOutput);
  const writeTerminal   = useTerminal((s) => s.writeTerminal);
  const resizeTerminal  = useTerminal((s) => s.resizeTerminal);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let unsubOutput;
    let onDataDisposable;
    let firstRenderDisposable;
    let openHandle = 0;          // rAF id for the deferred term.open()
    let cancelled = false;       // set true on cleanup so deferred opens bail
    let pasteListenerTarget = null; // helper textarea we attached the paste listener to
    // Set by our paste-event listener whenever it handles a paste, so the
    // keydown handler can tell whether the browser delivered a paste event
    // and decide if it needs to fall back to navigator.clipboard.
    let lastPasteEventAt = 0;
    const onTextareaPaste = (e) => {
      // Capture-phase listener: runs BEFORE xterm.js's own paste listener.
      // We stopImmediatePropagation to keep xterm's listener from also
      // pasting the text and causing a double-write to the PTY.
      e.preventDefault();
      e.stopImmediatePropagation();
      lastPasteEventAt = Date.now();
      const term = termRef.current;
      if (!term) return;
      const session = useTerminal.getState().sessions.find((s) => s.id === sessionId);
      handleTerminalPaste(term, session?.cwd, e.clipboardData);
    };

    // React StrictMode mounts the effect, runs cleanup, then mounts again. If
    // we call term.open() synchronously the first pass, xterm's Viewport
    // schedules an internal setTimeout that fires AFTER our cleanup disposes
    // the terminal, then crashes reading renderService.dimensions on the
    // disposed renderer. Deferring open() into a rAF lets cleanup cancel it
    // before xterm has scheduled anything internal.
    const initialize = () => {
      if (readyRef.current || cancelled) return;
      const { width, height } = container.getBoundingClientRect();
      if (width === 0 || height === 0) return; // still hidden — wait for resize event

      readyRef.current = true;

      const term = new Terminal({
        fontFamily: TERMINAL_FONT_FAMILY,
        fontSize: 13,
        // 1.0 matches what most CLI logos (block-character pixel art like
        // claude's mascot) are designed against. The default 1.2 stretches
        // cells vertically and distorts the logo.
        lineHeight: 1.0,
        cursorBlink: true,
        theme: {
          background:          '#0a0a0a',
          foreground:          '#e5e5e5',
          cursor:              '#e5e5e5',
          selectionBackground: '#264f78',
          ...TERMINAL_PALETTE,
        },
        scrollback: 5000,
        // Tell xterm.js the host PTY is Windows ConPTY. Without this it
        // doesn't apply the ConPTY-specific cursor / line-wrap workarounds,
        // and TUIs that use frame-redraw patterns (codex, gum, anything
        // built on react-ink) appear to shift left/right during streaming
        // because xterm renders the intermediate cursor positions of each
        // partial write instead of the synchronized end-of-frame state.
        ...(navigator.userAgent.includes('Windows')
          ? { windowsPty: { backend: 'conpty' } }
          : {}),
        // Don't double-convert line endings. ConPTY already emits proper
        // \r\n; with this on, xterm sees \r\n\n and inserts spurious blank
        // lines, which forces TUIs to redraw at shifted positions.
        convertEol: false,
        // Disable smooth-scroll animation. When a TUI hammers writes that
        // also scroll the viewport, the in-flight scroll animation can
        // interfere with the next frame's positioning.
        smoothScrollDuration: 0,
        allowProposedApi: true,
      });

      const fit = new FitAddon();
      term.loadAddon(fit);
      termRef.current = term;
      fitRef.current  = fit;

      // Wait for the terminal font to actually be loaded before opening the
      // terminal. xterm.js measures the font's cell width at open() time; if
      // the @font-face is still loading we get the fallback's metrics (often
      // a different width), the cell grid is sized wrong, and Unicode block
      // characters used in CLI logos (▀▄█) render misaligned. document.fonts
      // resolves immediately when the font is already loaded, so this is a
      // no-op on subsequent terminal opens.
      const fontReady = (typeof document !== 'undefined' && document.fonts?.ready)
        ? document.fonts.ready
        : Promise.resolve();

      fontReady.then(() => {
        if (cancelled) return;
        openHandle = requestAnimationFrame(() => {
        openHandle = 0;
        if (cancelled) {
          // StrictMode cleanup beat us to it. Dispose the unopened terminal —
          // since open() never ran, no internal setTimeout was scheduled, so
          // dispose is safe and won't leave a dangling refresh callback.
          try { term.dispose(); } catch (_) {}
          termRef.current = null;
          fitRef.current = null;
          readyRef.current = false;
          return;
        }
        try {
          term.open(container);
        } catch (e) {
          // eslint-disable-next-line no-console
          console.error('[terminal] term.open() failed', e);
          return;
        }

        // Load the WebGL renderer. xterm.js's default DOM renderer repaints
        // each row as an individual element, which flickers visibly when a
        // TUI (codex, top, htop, anything with high-frequency redraws)
        // hammers the terminal with many small writes per frame. WebGL is
        // GPU-accelerated and batches everything into one canvas paint per
        // animation frame, eliminating the flicker. Must be loaded AFTER
        // term.open() — the addon attaches to the live canvas element.
        // If WebGL isn't available (driver/context issue) we silently fall
        // back to the DOM renderer rather than failing the whole terminal.
        //
        // In production builds, the WebGL addon can fail asynchronously
        // during initialization if the canvas isn't fully ready. Schedule
        // it in a microtask to let the browser settle the terminal layout
        // first, and suppress any late-arriving errors to avoid console spam.
        Promise.resolve().then(() => {
          if (cancelled) return;
          try {
            const webgl = new WebglAddon();
            webgl.onContextLoss(() => {
              // GPU context can be lost on driver crash / GPU mode switch.
              // Dispose so xterm falls back to the DOM renderer cleanly
              // instead of rendering nothing on a dead canvas.
              try { webgl.dispose(); } catch (_) {}
            });
            term.loadAddon(webgl);
          } catch (err) {
            // eslint-disable-next-line no-console
            console.warn('[terminal] WebGL renderer unavailable, using DOM fallback:', err);
          }
        });

        // Don't call fit.fit() synchronously — xterm's WebGL/Canvas renderer
        // initialises asynchronously. Calling fit before it's ready triggers
        // the "dimensions undefined" RenderService crash. Wait for the first
        // rendered frame instead, which only fires once the renderer is live.
        firstRenderDisposable = term.onRender(() => {
          firstRenderDisposable?.dispose();
          firstRenderDisposable = null;
          try { fitRef.current?.fit(); } catch (_) {}
          const t = termRef.current;
          if (t?.cols > 0 && t?.rows > 0) resizeTerminal(sessionId, t.cols, t.rows);
        });

        // Suppress xterm's translation of Ctrl+V / Ctrl+C into raw ^V / ^C.
        // Ctrl+V is then handled by the capture-phase paste listener below
        // (which gives us access to clipboardData's MIME types — needed for
        // image-vs-text detection). Ctrl+C still falls through as ^C when
        // there's no selection, so SIGINT keeps working.
        term.attachCustomKeyEventHandler((e) => {
          if (e.type !== 'keydown') return true;
          const mod = e.ctrlKey || e.metaKey;
          if (!mod || e.altKey) return true;
          const k = e.key.toLowerCase();

          if (k === 'v') {
            // Primary paste path is the paste-event listener on the textarea
            // (it has synchronous access to image MIME types, which the
            // navigator.clipboard API can't easily match). We only fall back
            // to navigator.clipboard if no paste event arrives within ~80ms —
            // a WebView2 focus quirk we've seen elsewhere. setTimeout(0)
            // gives the browser's paste event a chance to fire first.
            const startedAt = Date.now();
            setTimeout(() => {
              if (lastPasteEventAt >= startedAt) return; // paste event handled it
              const session = useTerminal.getState().sessions.find((s) => s.id === sessionId);
              pasteViaClipboardApi(term, session?.cwd);
            }, 80);
            return false;
          }

          if (k === 'c') {
            // Ctrl+Shift+C → always copy (no SIGINT fallback).
            // Ctrl+C → copy iff there's a selection, else let xterm send ^C.
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

        unsubOutput = subscribeOutput(sessionId, (data) => {
          if (typeof data === 'string')      term.write(data);
          else if (data instanceof Uint8Array) term.write(data);
          else if (Array.isArray(data))        term.write(new Uint8Array(data));
        });

        onDataDisposable = term.onData((d) => writeTerminal(sessionId, d));

        // Install our paste handler on xterm's hidden helper textarea in
        // capture phase. It does the actual paste (image-first, text fall-
        // back via handleTerminalPaste) AND stopImmediatePropagation's
        // xterm's own paste listener so we don't double-write to the PTY.
        const helperTextarea = container.querySelector('textarea');
        if (helperTextarea) {
          pasteListenerTarget = helperTextarea;
          helperTextarea.addEventListener('paste', onTextareaPaste, true);
        }

        if (term.cols > 0 && term.rows > 0) {
          resizeTerminal(sessionId, term.cols, term.rows);
        }
        });
      });
    };

    // ResizeObserver serves double duty:
    //   • First observation with non-zero size → triggers initialize()
    //   • Subsequent observations → fit the already-open terminal
    //
    // We track the last cols/rows we sent to the PTY and skip the resize IPC
    // if the new fit produces the same numbers. Without this, sub-pixel
    // container changes (anything that mutates the canvas during streaming —
    // glyph atlas growth, scrollbar appearance, etc.) can re-trigger fit()
    // and send a redundant resize signal to the PTY, which makes TUIs like
    // codex re-render their bottom panel at a shifted offset.
    let lastCols = 0;
    let lastRows = 0;
    const ro = new ResizeObserver(() => {
      if (!readyRef.current) {
        initialize();
        return;
      }
      // Skip fit when the container is hidden (0×0 — happens when this
      // terminal isn't the active tab). FitAddon falls back to MINIMUM_COLS
      // (~2) in that case and resizes the PTY narrow; the shell reflows its
      // output at that width and the narrow lines stay stuck in scrollback
      // even after we restore the correct size on tab focus.
      const { width, height } = container.getBoundingClientRect();
      if (width === 0 || height === 0) return;

      const fit  = fitRef.current;
      const term = termRef.current;
      if (!fit || !term) return;
      try {
        fit.fit();
        if (term.cols > 0 && term.rows > 0 &&
            (term.cols !== lastCols || term.rows !== lastRows)) {
          lastCols = term.cols;
          lastRows = term.rows;
          resizeTerminal(sessionId, term.cols, term.rows);
        }
      } catch (_) {}
    });

    ro.observe(container);

    // Also attempt immediately in case the container is already visible
    // (e.g. this terminal is the active tab when it first mounts).
    initialize();

    return () => {
      cancelled = true;
      readyRef.current = false;
      if (openHandle) {
        cancelAnimationFrame(openHandle);
        openHandle = 0;
      }
      firstRenderDisposable?.dispose(); // cancel if component unmounts before first render
      ro.disconnect();
      onDataDisposable?.dispose();
      unsubOutput?.();
      if (pasteListenerTarget) {
        pasteListenerTarget.removeEventListener('paste', onTextareaPaste, true);
        pasteListenerTarget = null;
      }
      // Defer dispose: xterm's Viewport constructor schedules setTimeout(0)
      // → rAF → _innerRefresh which reads renderService.dimensions. Disposing
      // synchronously nulls renderer.value and the pending rAF crashes. Two
      // animation frames + a microtask is enough for those callbacks to drain
      // on the still-live renderer before we tear it down.
      const term = termRef.current;
      termRef.current = null;
      fitRef.current  = null;
      if (term) {
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            try { term.dispose(); } catch (_) {}
          });
        });
      }
    };
  // sessionId is the only real dep — subscribeOutput/writeTerminal/resizeTerminal
  // are stable Zustand references and don't change between renders.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // When this tab becomes the active/visible one, re-fit if already initialised.
  // If not yet initialised the ResizeObserver will handle it when the container
  // goes from display:none → display:block and gains layout dimensions.
  useEffect(() => {
    if (!active) return;
    
    // Double-RAF ensures the browser has fully reflowed the layout after
    // removing display:none from the container. Single RAF can run before
    // layout settles, causing fit() to measure stale/zero dimensions and
    // leaving the terminal with incorrect cols/rows (manifests as text
    // wrapping at a narrow width even though the container is wide).
    let id1, id2;
    id1 = requestAnimationFrame(() => {
      id2 = requestAnimationFrame(() => {
        const container = containerRef.current;
        const fit  = fitRef.current;
        const term = termRef.current;
        if (!container || !fit || !term) return;
        
        // Safety check: ensure container has non-zero dimensions before fitting.
        // If still zero, the ResizeObserver will handle it when layout settles.
        const { width, height } = container.getBoundingClientRect();
        if (width === 0 || height === 0) return;
        
        try {
          fit.fit();
          if (term.cols > 0 && term.rows > 0) {
            resizeTerminal(sessionId, term.cols, term.rows);
          }
        } catch (_) {}
      });
    });
    return () => {
      if (id1) cancelAnimationFrame(id1);
      if (id2) cancelAnimationFrame(id2);
    };
  }, [active, sessionId, resizeTerminal]);

  return <div ref={containerRef} className="h-full w-full overflow-hidden" />;
}
