import React, { useEffect, useRef } from 'react';
import { Terminal } from 'xterm';
import { FitAddon } from '@xterm/addon-fit';
import 'xterm/css/xterm.css';
import { useTerminal } from '@/state/terminal';

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

    // Only call term.open() once the container has real layout dimensions.
    // Calling it on a display:none element makes xterm's Viewport crash
    // because it can't measure scroll dimensions.
    const initialize = () => {
      if (readyRef.current) return;
      const { width, height } = container.getBoundingClientRect();
      if (width === 0 || height === 0) return; // still hidden — wait for resize event

      readyRef.current = true;

      const term = new Terminal({
        fontFamily: 'Consolas, "JetBrains Mono", monospace',
        fontSize: 12,
        cursorBlink: true,
        theme: {
          background: '#0a0a0a',
          foreground: '#e5e5e5',
          cursor: '#e5e5e5',
          selectionBackground: '#264f78',
        },
        scrollback: 5000,
        convertEol: true,
        allowProposedApi: true,
      });

      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(container);
      termRef.current = term;
      fitRef.current  = fit;

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

      unsubOutput = subscribeOutput(sessionId, (data) => {
        if (typeof data === 'string')      term.write(data);
        else if (data instanceof Uint8Array) term.write(data);
        else if (Array.isArray(data))        term.write(new Uint8Array(data));
      });

      onDataDisposable = term.onData((d) => writeTerminal(sessionId, d));

      if (term.cols > 0 && term.rows > 0) {
        resizeTerminal(sessionId, term.cols, term.rows);
      }
    };

    // ResizeObserver serves double duty:
    //   • First observation with non-zero size → triggers initialize()
    //   • Subsequent observations → fit the already-open terminal
    const ro = new ResizeObserver(() => {
      if (!readyRef.current) {
        initialize();
      } else {
        const fit  = fitRef.current;
        const term = termRef.current;
        if (!fit || !term) return;
        try {
          fit.fit();
          if (term.cols > 0 && term.rows > 0) {
            resizeTerminal(sessionId, term.cols, term.rows);
          }
        } catch (_) {}
      }
    });

    ro.observe(container);

    // Also attempt immediately in case the container is already visible
    // (e.g. this terminal is the active tab when it first mounts).
    initialize();

    return () => {
      readyRef.current = false;
      firstRenderDisposable?.dispose(); // cancel if component unmounts before first render
      ro.disconnect();
      onDataDisposable?.dispose();
      unsubOutput?.();
      termRef.current?.dispose();
      termRef.current = null;
      fitRef.current  = null;
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
    const id = requestAnimationFrame(() => {
      const fit  = fitRef.current;
      const term = termRef.current;
      if (!fit || !term) return;
      try {
        fit.fit();
        if (term.cols > 0 && term.rows > 0) {
          resizeTerminal(sessionId, term.cols, term.rows);
        }
      } catch (_) {}
    });
    return () => cancelAnimationFrame(id);
  }, [active, sessionId, resizeTerminal]);

  return <div ref={containerRef} className="h-full w-full overflow-hidden" />;
}
