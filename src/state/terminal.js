import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { uiStore } from './ui.js';

export const terminalStore = createStore({
  sessions: [],        // Array of { id, label, cwd, is_agent }
  activeSessionId: null,
  splitSessionIds: [], // Session IDs visible side-by-side in the terminal split
  availableShells: [], // Array of { name, path, is_default }
  defaultShellPath: null, // Path to the default shell
});

/** Load available shells from the backend. Called once on startup. */
export async function loadAvailableShells() {
  try {
    const shells = await api.detectShells();
    if (shells && shells.length > 0) {
      const defaultShell = shells.find(s => s.is_default);
      terminalStore.setState({
        availableShells: shells,
        defaultShellPath: defaultShell ? defaultShell.path : null,
      });
    }
  } catch (e) {
    console.error('Failed to detect shells:', e);
  }
}

/** Set which shell should be used by default for new terminals. */
export function setDefaultShell(shellPath) {
  terminalStore.setState({ defaultShellPath: shellPath });
}

export async function createTerminal(cwd, label, shellProgram) {
  try {
    // Use specified shell, or the user-selected default, or null (system default)
    const shell = shellProgram || terminalStore.getState('defaultShellPath') || null;

    // Derive label from the shell being used if none explicitly provided
    if (!label && shell) {
      const shells = terminalStore.getState('availableShells');
      const matched = shells.find(s => s.path === shell);
      if (matched) label = matched.name;
    }

    const info = await api.createTerminal(cwd || null, label || null, false, shell);
    if (!info) return null;

    const sessions = [...terminalStore.getState('sessions'), info];
    // Always switch the split view to the new terminal so it's immediately visible
    terminalStore.setState({ sessions, activeSessionId: info.id, splitSessionIds: [info.id] });

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

  const splitIds = terminalStore.getState('splitSessionIds').filter(id => id !== sessionId);
  terminalStore.setState({ sessions, activeSessionId: newActiveId, splitSessionIds: splitIds });
}

export function setActiveSession(sessionId) {
  terminalStore.setState({ activeSessionId: sessionId });
  // Also make sure this session is in the split view
  const splitIds = terminalStore.getState('splitSessionIds');
  if (!splitIds.includes(sessionId)) {
    terminalStore.setState({ splitSessionIds: [sessionId] });
  }
}

/**
 * Split the terminal: create a new terminal session side-by-side.
 */
export async function splitTerminal(cwd) {
  // Capture current split before createTerminal resets it
  const currentSplit = terminalStore.getState('splitSessionIds');
  const info = await createTerminal(cwd);
  if (info) {
    const splitIds = [...currentSplit, info.id];
    terminalStore.setState({ splitSessionIds: splitIds, activeSessionId: info.id });
  }
  return info;
}
