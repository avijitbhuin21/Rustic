import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

let listenersWired = false;
const outputSubscribers = new Map();

export const useTerminal = create((set, get) => ({
  sessions: [],
  activeSessionId: null,
  shells: [],

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
      set((s) => ({
        sessions,
        // SessionInfo serialises its id as `id` (not `session_id`)
        activeSessionId: sessions.find((x) => x.id === s.activeSessionId)
          ? s.activeSessionId
          : sessions[0]?.id ?? null,
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
      activeSessionId: info.id,
    }));
    return info;
  },

  closeTerminal: async (sessionId) => {
    try { await invoke('close_terminal', { sessionId }); } catch {}
    set((s) => {
      const sessions = s.sessions.filter((x) => x.id !== sessionId);
      return {
        sessions,
        activeSessionId: s.activeSessionId === sessionId
          ? sessions[0]?.id ?? null
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

  subscribeOutput: (sessionId, fn) => {
    outputSubscribers.set(sessionId, fn);
    return () => {
      if (outputSubscribers.get(sessionId) === fn) outputSubscribers.delete(sessionId);
    };
  },
}));
