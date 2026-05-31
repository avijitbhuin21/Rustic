import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  makeLeaf,
  splitAt,
  removeSession as removeSplitSession,
  setSizes as setSplitSizes,
  pruneDeadSessions,
} from '@/lib/split-tree';
import {
  disposeTerminalInstance,
  reconcileTerminalInstances,
} from '@/components/terminal/terminal-instance';

let listenersWired = false;
const outputSubscribers = new Map();

// Layout mode persists across restarts (it's a user preference, not tied to a
// specific session). Terminal *order* and split structure are NOT persisted:
// session ids are backend-assigned and reset every launch, and there's no
// PTY-restore, so the terminals themselves don't survive a restart. Persisting
// ids would key on values that no longer exist. Order/splits live for the run.
const LAYOUT_MODE_KEY = 'rustic.terminal.layoutMode';
// Only two modes survive: 'tabs' (one visible pane) and 'grid' (all terminals
// stacked full-width in a single scrollable column). 'row' and 'split' were
// removed; a persisted value of either migrates to 'tabs' on load (handled by
// the `has()` fallback below).
const VALID_LAYOUT_MODES = new Set(['tabs', 'grid']);

function loadLayoutMode() {
  try {
    const v = localStorage.getItem(LAYOUT_MODE_KEY);
    return VALID_LAYOUT_MODES.has(v) ? v : 'tabs';
  } catch {
    return 'tabs';
  }
}

export const useTerminal = create((set, get) => ({
  sessions: [],
  activeSessionId: null,
  shells: [],
  // Track which terminals are hidden (not terminated, just not visible).
  // Hidden terminals keep running in the background.
  hiddenSessionIds: new Set(),
  // Explicit user-defined ordering of terminal tabs (session ids). New
  // terminals append; closed terminals are pruned; drag-drop rewrites it.
  // Any live session missing from this list is treated as appended at the end
  // (see `orderedSessions`), so the backend can add sessions we haven't seen.
  order: [],
  // Content-area layout: 'tabs' (one visible pane) or 'grid' (all terminals
  // stacked full-width in a scrollable column). Persisted across restarts.
  layoutMode: loadLayoutMode(),
  // Split layout tree (see lib/split-tree.js). Split mode was removed from the
  // UI; the tree machinery stays so the helpers/actions remain valid, but it's
  // no longer reachable. Lives for the run only.
  splitTree: null,

  setLayoutMode: (mode) => {
    if (!VALID_LAYOUT_MODES.has(mode)) return;
    try { localStorage.setItem(LAYOUT_MODE_KEY, mode); } catch {}
    set({ layoutMode: mode });
  },

  // Seed the split tree with a single leaf if it's empty. Used when split mode
  // is restored from a previous run (layoutMode persists, the tree doesn't).
  ensureSplitTree: (sessionId) =>
    set((s) => (s.splitTree ? {} : { splitTree: makeLeaf(sessionId) })),

  // Split the pane holding `targetSessionId`, inserting `newSessionId` on the
  // given side ('left' | 'right' | 'top' | 'bottom').
  splitPane: (targetSessionId, newSessionId, placement) =>
    set((s) => {
      const tree = s.splitTree ?? makeLeaf(targetSessionId);
      return {
        splitTree: splitAt(tree, targetSessionId, newSessionId, placement),
        activeSessionId: newSessionId,
      };
    }),

  // Remove a pane from the split tree (does NOT terminate the terminal).
  removeSplitPane: (sessionId) =>
    set((s) => ({ splitTree: removeSplitSession(s.splitTree, sessionId) })),

  // Persist a split node's child sizes after a divider drag.
  resizeSplit: (nodeId, sizes) =>
    set((s) => ({ splitTree: setSplitSizes(s.splitTree, nodeId, sizes) })),

  // Rewrite the tab order from a drag-drop result. `ids` is the new ordering
  // of the currently-listed session ids. Ids already tracked but not in this
  // list (e.g. a hidden terminal) are preserved after them so unhiding doesn't
  // lose their place.
  reorderTerminals: (ids) =>
    set((s) => {
      const inList = new Set(ids);
      const rest = s.order.filter((id) => !inList.has(id));
      return { order: [...ids, ...rest] };
    }),

  wireListeners: async () => {
    if (listenersWired) return;
    listenersWired = true;
    await listen('terminal-output', (e) => {
      // Backend emits snake_case event payload: { session_id, data }
      const { session_id, data } = e.payload ?? {};
      const fn = outputSubscribers.get(session_id);
      if (fn) fn(data);
    });
    await listen('terminal-list-changed', () => {
      get().refreshSessions();
    });
  },

  refreshSessions: async () => {
    try {
      const sessions = await invoke('list_terminals');
      const liveIds = new Set(sessions.map((x) => x.id));
      // Free xterm instances (and their scrollback) for any terminal the
      // backend no longer reports — i.e. it died/exited. Live terminals keep
      // their instance regardless of which layout mode is showing them.
      reconcileTerminalInstances(liveIds);
      set((s) => ({
        sessions,
        // Drop ids for sessions the backend no longer reports, but preserve the
        // user's ordering of the survivors. New ids are appended by
        // `orderedSessions` until a drag rewrites the list.
        order: s.order.filter((id) => liveIds.has(id)),
        // Prune split-tree leaves whose terminal has died, collapsing splits.
        splitTree: s.splitTree ? pruneDeadSessions(s.splitTree, liveIds) : null,
        // SessionInfo serialises its id as `id` (not `session_id`). Keep the
        // current active terminal if it's still alive; otherwise fall back to
        // the first *user* terminal — never auto-activate an agent terminal,
        // which would otherwise yank the pane onto an agent's shell the moment
        // the agent runs a command.
        activeSessionId: sessions.find((x) => x.id === s.activeSessionId)
          ? s.activeSessionId
          : sessions.find((x) => !x.is_agent)?.id ?? null,
      }));
    } catch {}
  },

  detectShells: async () => {
    try {
      const shells = await invoke('detect_shells');
      set({ shells });
    } catch {}
  },

  createTerminal: async ({ cwd, label, shellProgram } = {}) => {
    const info = await invoke('create_terminal', {
      cwd: cwd ?? null,
      label: label ?? null,
      isAgent: false,
      shellProgram: shellProgram ?? null,
    });
    set((s) => ({
      sessions: [...s.sessions, info],
      order: s.order.includes(info.id) ? s.order : [...s.order, info.id],
      activeSessionId: info.id,
    }));
    return info;
  },

  hideTerminal: (sessionId) => {
    set((s) => {
      const hiddenSessionIds = new Set(s.hiddenSessionIds);
      hiddenSessionIds.add(sessionId);
      const sessions = s.sessions.filter((x) => !hiddenSessionIds.has(x.id));
      const visibleSessions = s.sessions.filter((x) => x.id !== sessionId && !hiddenSessionIds.has(x.id));
      return {
        hiddenSessionIds,
        // Never auto-activate an agent terminal on fallback — they only
        // surface in the pane when the user explicitly opens one. Falling
        // back to sessions[0] would promote the next lingering agent
        // terminal into view, so closing one appears to summon another.
        activeSessionId: s.activeSessionId === sessionId
          ? visibleSessions.find((x) => !x.is_agent)?.id ?? null
          : s.activeSessionId,
      };
    });
  },

  showTerminal: (sessionId) => {
    set((s) => {
      const hiddenSessionIds = new Set(s.hiddenSessionIds);
      hiddenSessionIds.delete(sessionId);
      return {
        hiddenSessionIds,
        activeSessionId: sessionId,
      };
    });
  },

  closeTerminal: async (sessionId) => {
    try { await invoke('close_terminal', { sessionId }); } catch {}
    // Free the xterm instance + its scrollback immediately on an explicit
    // close (the only path that should clear history / release memory).
    disposeTerminalInstance(sessionId);
    set((s) => {
      const sessions = s.sessions.filter((x) => x.id !== sessionId);
      const hiddenSessionIds = new Set(s.hiddenSessionIds);
      hiddenSessionIds.delete(sessionId);
      return {
        sessions,
        order: s.order.filter((id) => id !== sessionId),
        splitTree: s.splitTree ? removeSplitSession(s.splitTree, sessionId) : null,
        hiddenSessionIds,
        // Fall back to the first *user* terminal, never an agent one — same
        // rule as refreshSessions. Using sessions[0] here would promote the
        // next lingering agent terminal into the active pane, which is why
        // closing one made another appear to "take its place".
        activeSessionId: s.activeSessionId === sessionId
          ? sessions.find((x) => !x.is_agent)?.id ?? null
          : s.activeSessionId,
      };
    });
  },

  setActiveSessionId: (id) => set({ activeSessionId: id }),

  writeTerminal: async (sessionId, text) => {
    await invoke('write_terminal', { sessionId, data: text });
  },

  resizeTerminal: async (sessionId, cols, rows) => {
    try { await invoke('resize_terminal', { sessionId, cols, rows }); } catch {}
  },

  // Capture a terminal's current visible screen as clean plain text (escape
  // codes resolved by the backend headless emulator). Used by the chat
  // composer to attach a terminal snapshot to a message.
  readTerminalScreen: async (sessionId) => {
    return invoke('read_terminal_screen', { sessionId });
  },

  subscribeOutput: (sessionId, fn) => {
    outputSubscribers.set(sessionId, fn);
    return () => {
      if (outputSubscribers.get(sessionId) === fn) outputSubscribers.delete(sessionId);
    };
  },
}));

/**
 * Order a list of sessions by the user's persisted tab order. Sessions present
 * in `order` come first in that order; any session not yet in `order` (e.g. a
 * backend-created one we haven't appended) follows in its original relative
 * position. Pure helper so components can derive the ordered list without
 * mutating state.
 */
export function orderedSessions(sessions, order) {
  const rank = new Map(order.map((id, i) => [id, i]));
  return [...sessions].sort((a, b) => {
    const ra = rank.has(a.id) ? rank.get(a.id) : Infinity;
    const rb = rank.has(b.id) ? rank.get(b.id) : Infinity;
    return ra - rb;
  });
}
