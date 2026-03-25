import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { uiStore } from './ui.js';

export const terminalStore = createStore({
  sessions: [],        // Array of { id, label, cwd, is_agent }
  activeSessionId: null,
});

export async function createTerminal(cwd, label) {
  try {
    const info = await api.createTerminal(cwd || null, label || null, false);
    if (!info) return null;

    const sessions = [...terminalStore.getState('sessions'), info];
    terminalStore.setState({ sessions, activeSessionId: info.id });

    // Show bottom panel
    uiStore.setState({ bottomPanelVisible: true });

    return info;
  } catch (e) {
    console.error('Failed to create terminal:', e);
    return null;
  }
}

export async function closeTerminal(sessionId) {
  try {
    await api.closeTerminal(sessionId);
  } catch (e) {
    console.error('Failed to close terminal:', e);
  }

  const sessions = terminalStore.getState('sessions').filter(s => s.id !== sessionId);
  const activeId = terminalStore.getState('activeSessionId');
  let newActiveId = null;

  if (activeId === sessionId) {
    newActiveId = sessions.length > 0 ? sessions[sessions.length - 1].id : null;
  } else {
    newActiveId = activeId;
  }

  terminalStore.setState({ sessions, activeSessionId: newActiveId });
}

export function setActiveSession(sessionId) {
  terminalStore.setState({ activeSessionId: sessionId });
}
