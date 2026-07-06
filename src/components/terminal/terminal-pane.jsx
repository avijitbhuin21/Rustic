import React, { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowUp, ArrowDown, X as XIcon } from 'lucide-react';
import '@xterm/xterm/css/xterm.css';
import { cn } from '@/lib/utils';
import { acquireTerminalInstance } from './terminal-instance';

// Highlight colors for search matches. `match*` styles every hit; `activeMatch*`
// styles the currently-focused one. Sourced from CSS variables so themes can
// override; the amber fallbacks stand apart from the blue selection color
// without clashing with the dark theme.
function cssColor(name, fallback) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function searchDecorations() {
  return {
    matchBackground: cssColor('--terminal-match-bg', '#5a4a2f'),
    matchBorder: cssColor('--terminal-match-border', '#e5b567'),
    matchOverviewRuler: cssColor('--terminal-match-border', '#e5b567'),
    activeMatchBackground: cssColor('--terminal-match-active-bg', '#e5b567'),
    activeMatchBorder: cssColor('--terminal-match-active-border', '#ffffff'),
    activeMatchColorOverviewRuler: cssColor('--terminal-match-active-border', '#ffffff'),
  };
}

// Find-in-terminal overlay. Drives the xterm SearchAddon (`searchRef`) — it
// searches xterm's live buffer (visible screen + scrollback) and paints
// decorations in place. `results` ({index,count}) comes from the addon's
// onDidChangeResults, surfaced via the persistent instance.
function TerminalSearchBar({ searchRef, termRef, results, onClose }) {
  const inputRef = useRef(null);
  const [query, setQuery] = useState('');
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const decorations = useMemo(() => searchDecorations(), []);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // Re-run the search incrementally as the query or options change. findNext
  // from the current position gives the familiar "jump to next as you type".
  useEffect(() => {
    const s = searchRef.current;
    if (!s) return;
    if (!query) {
      s.clearDecorations?.();
      return;
    }
    try {
      s.findNext(query, {
        decorations,
        caseSensitive,
        regex: useRegex,
      });
    } catch (_) {
      // Invalid regex while typing — ignore until it parses.
    }
  }, [query, caseSensitive, useRegex, searchRef, decorations]);

  const step = (dir) => {
    const s = searchRef.current;
    if (!s || !query) return;
    const opts = { decorations, caseSensitive, regex: useRegex };
    try {
      if (dir === 'prev') s.findPrevious(query, opts);
      else s.findNext(query, opts);
    } catch (_) {}
  };

  const close = () => {
    searchRef.current?.clearDecorations?.();
    onClose();
    // Return focus to the terminal so typing resumes immediately.
    requestAnimationFrame(() => termRef.current?.focus());
  };

  const onKeyDown = (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      step(e.shiftKey ? 'prev' : 'next');
    }
  };

  const countLabel = !query
    ? ''
    : results.count === 0
      ? 'No results'
      : `${results.index + 1}/${results.count}`;

  const toggleCls = (on) =>
    cn(
      'rounded px-1 text-[11px] font-medium leading-none',
      on ? 'bg-primary/30 text-foreground' : 'text-muted-foreground hover:text-foreground'
    );

  return (
    <div
      className="absolute right-2 top-2 z-20 flex items-center gap-1 rounded-md border border-border/70 bg-popover/95 px-1.5 py-1 shadow-md backdrop-blur"
      // Keep clicks inside the bar from bubbling to the terminal (which would
      // steal focus / deselect).
      onMouseDown={(e) => e.stopPropagation()}
    >
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Find"
        spellCheck={false}
        className="h-6 w-40 bg-transparent text-xs text-foreground outline-none placeholder:text-muted-foreground"
      />
      <span className="min-w-[44px] text-right text-[11px] tabular-nums text-muted-foreground">
        {countLabel}
      </span>
      <button onClick={() => setCaseSensitive((v) => !v)} className={toggleCls(caseSensitive)} title="Match case">
        Aa
      </button>
      <button onClick={() => setUseRegex((v) => !v)} className={toggleCls(useRegex)} title="Use regular expression">
        .*
      </button>
      <button
        onClick={() => step('prev')}
        className="rounded p-0.5 text-muted-foreground hover:bg-muted/60 hover:text-foreground"
        title="Previous match (Shift+Enter)"
      >
        <ArrowUp className="size-3.5" />
      </button>
      <button
        onClick={() => step('next')}
        className="rounded p-0.5 text-muted-foreground hover:bg-muted/60 hover:text-foreground"
        title="Next match (Enter)"
      >
        <ArrowDown className="size-3.5" />
      </button>
      <button
        onClick={close}
        className="rounded p-0.5 text-muted-foreground hover:bg-destructive/20 hover:text-destructive"
        title="Close (Esc)"
      >
        <XIcon className="size-3.5" />
      </button>
    </div>
  );
}

/**
 * Thin wrapper around a persistent terminal instance (see terminal-instance.js).
 * It does NOT own the xterm instance or its buffer — it merely reparents the
 * instance's DOM element into its mount node, so the terminal's full history
 * survives every remount (fullscreen toggle, chat-dock toggle, layout changes).
 * The xterm instance is created on first use and disposed only when the PTY is
 * closed.
 */
export function TerminalPane({ sessionId, active }) {
  const mountRef = useRef(null);
  const instRef = useRef(null);
  const searchRef = useRef(null); // points at the instance's SearchAddon
  const termRef = useRef(null);   // points at the instance's Terminal

  const [searchOpen, setSearchOpen] = useState(false);
  const [searchResults, setSearchResults] = useState({ index: -1, count: 0 });

  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;

    const inst = acquireTerminalInstance(sessionId);
    instRef.current = inst;

    // Reparent the persistent element into this mount node. Moving a DOM node
    // preserves its descendants, and the scrollback lives in the JS instance,
    // so nothing is lost across remounts.
    mount.appendChild(inst.container);

    const unsubSearch = inst.subscribeSearchResults(setSearchResults);
    inst.setOnOpenSearch(() => {
      searchRef.current = inst.search;
      termRef.current = inst.term;
      setSearchOpen(true);
    });

    // Open (if first time) + fit now that the element is attached and sized.
    inst.attach();

    return () => {
      unsubSearch();
      inst.setOnOpenSearch(null);
      // Detach but DO NOT dispose — the instance and its history persist.
      if (inst.container.parentNode === mount) {
        mount.removeChild(inst.container);
      }
    };
  }, [sessionId]);

  // When this pane becomes the active/visible one, re-fit. Double-rAF lets the
  // browser finish reflowing after a display:none → block transition so fit()
  // measures real dimensions instead of stale/zero ones.
  useEffect(() => {
    if (!active) return;
    let id1, id2;
    id1 = requestAnimationFrame(() => {
      id2 = requestAnimationFrame(() => {
        instRef.current?.refit();
        // Coming back to a terminal whose canvas was display:none: rebuild the
        // WebGL glyph atlas + full repaint so a stale atlas from the hidden
        // stint can't garble the screen or scrollback.
        instRef.current?.repaint();
      });
    });
    return () => {
      if (id1) cancelAnimationFrame(id1);
      if (id2) cancelAnimationFrame(id2);
    };
  }, [active, sessionId]);

  return (
    <div className="relative h-full w-full overflow-hidden">
      <div ref={mountRef} className="h-full w-full overflow-hidden" />
      {searchOpen && (
        <TerminalSearchBar
          searchRef={searchRef}
          termRef={termRef}
          results={searchResults}
          onClose={() => setSearchOpen(false)}
        />
      )}
    </div>
  );
}
