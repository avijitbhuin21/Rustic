import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { IS_WEB } from '@/lib/platform';

// On the web target, terminal keystrokes go up the already-open WebSocket
// (one round-trip on the live socket) instead of a fresh HTTP POST per char.
// Loaded lazily so the desktop bundle never pulls the web transport; the first
// keystrokes before it resolves harmlessly fall back to the HTTP path below.
let wsTerminalSend = null;
if (IS_WEB) {
  import('@/lib/web/transport-core.js')
    .then((m) => { wsTerminalSend = m.sendTerminalInput; })
    .catch(() => {});
}
import {
  disposeTerminalInstance,
  reconcileTerminalInstances,
} from '@/components/terminal/terminal-instance';

let listenersWired = false;
const outputSubscribers = new Map();

// Terminal *order* is NOT persisted: session ids are backend-assigned and reset
// every launch, and there's no PTY-restore, so the terminals themselves don't
// survive a restart. Persisting ids would key on values that no longer exist.
// Order lives for the run only.

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
    // Fast path: push over the live WS. Falls back to HTTP when the socket
    // isn't open yet (or on desktop, where wsTerminalSend stays null).
    if (wsTerminalSend && wsTerminalSend(sessionId, text)) return;
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

  // Fetch a session's full retained raw-output buffer (ANSI intact). Used to
  // replay scrollback into a freshly-mounted xterm so terminals opened AFTER
  // output was produced (e.g. agent-spawned ones) aren't blank.
  readTerminalBuffer: async (sessionId) => {
    try {
      return await invoke('read_terminal_buffer', { sessionId });
    } catch {
      return '';
    }
  },

  // Clean, de-duplicated scrollback for rehydrating an xterm instance: the
  // backend serializes the headless emulator's resolved grid (history + screen)
  // as ANSI, so ConPTY repaint/resize frames don't reappear as duplicate
  // scrollback lines the way replaying `read_terminal_buffer` (raw bytes) does.
  // Falls back to the raw buffer if the scrollback command is unavailable (e.g.
  // an older backend), so a terminal is never left blank.
  readTerminalScrollback: async (sessionId) => {
    try {
      return await invoke('read_terminal_scrollback', { sessionId });
    } catch {
      try {
        return await invoke('read_terminal_buffer', { sessionId });
      } catch {
        return '';
      }
    }
  },

  subscribeOutput: (sessionId, fn) => {
    outputSubscribers.set(sessionId, fn);
    return () => {
      if (outputSubscribers.get(sessionId) === fn) outputSubscribers.delete(sessionId);
    };
  },
}));

/**
 * Caption for a terminal tab/tile/header: the project (or agent) label plus the
 * shell PID, e.g. "Rustic · 12345". The PID disambiguates multiple terminals
 * opened on the same project and lets the user reference a specific shell.
 */
export function terminalTabLabel(session) {
  if (!session) return '';
  const base = session.label || `pty ${session.id}`;
  return session.pid ? `${base} · ${session.pid}` : base;
}

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
