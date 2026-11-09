// Session persistence for "Restore last session" (Settings › General).
//
// A snapshot of the open file tabs (per split group), the active chat/project
// and the chat-dock state is written to localStorage on every change. On the
// next launch — if the setting is on — it is replayed once projects and
// settings have loaded. Saving only starts AFTER the restore attempt so the
// empty startup state can't overwrite the previous session first.

import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { useEditor } from '@/state/editor';
import { useAgent } from '@/state/agent';
import { useExternalAgents } from '@/state/external-agents';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useSettings } from '@/state/settings';

const KEY = 'rustic.session.v1';
const SAVE_DEBOUNCE_MS = 400;
const RESTORE_INDICATOR_MAX_MS = 20000;

let persistenceStarted = false;
let restoreAttempted = false;
// projectId → last chat open in that project; survives project switches (which
// clear activeTaskId) so every project's chat comes back, not just the active one.
let chatsByProject = {};
// Saved chats not yet reopened: applied the first time each project is visited.
let pendingChats = {};

/** Project a chat belongs to, from the loaded task lists (null when unknown). */
function projectOfTask(taskId) {
  const { tasksByProject, tasks } = useAgent.getState();
  for (const [pid, list] of Object.entries(tasksByProject ?? {})) {
    if (list?.some((t) => t.id === taskId)) return pid;
  }
  return (tasks ?? []).find((t) => t.id === taskId)?.project_id ?? null;
}

/** Record the open chat under the project it belongs to. */
function rememberActiveChat() {
  const { activeTaskId } = useAgent.getState();
  if (!activeTaskId || /^(local|mock)-/.test(activeTaskId)) return;
  // The agent store's activeProject lags the explorer during startup, so
  // attribute the chat by its own project_id, never by "whatever is active".
  const pid = projectOfTask(activeTaskId);
  if (!pid || chatsByProject[pid] === activeTaskId) return;
  chatsByProject = { ...chatsByProject, [pid]: activeTaskId };
  delete pendingChats[pid];
}

/** Reopen a project's saved chat on its first visit, if the chat still exists. */
async function reopenPendingChat(projectId) {
  const taskId = pendingChats[projectId];
  if (!taskId || useExplorer.getState().activeProjectId !== projectId) return;
  delete pendingChats[projectId];
  const tasks = await useAgent.getState().loadTasksForProject(projectId);
  const agent = useAgent.getState();
  if (!tasks.some((t) => t.id === taskId)) {
    const { [projectId]: _gone, ...rest } = chatsByProject;
    chatsByProject = rest;
    return;
  }
  if (agent.activeProject?.id === projectId && !agent.activeTaskId) agent.setActiveTask(taskId);
}

/** True when the last saved session had restore enabled and something to reopen. */
function savedSessionWillRestore() {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) || 'null');
    if (!saved || saved.restore !== true) return false;
    const hasTabs = Array.isArray(saved.groups) && saved.groups.some((g) => g?.tabs?.length);
    return hasTabs || !!saved.activeTaskId;
  } catch {
    return false;
  }
}

/** `restoring` is true from launch until the previous session has been reopened (drives the loading indicator). */
export const useSessionRestore = create(() => ({ restoring: savedSessionWillRestore() }));

if (useSessionRestore.getState().restoring) {
  setTimeout(() => useSessionRestore.setState({ restoring: false }), RESTORE_INDICATOR_MAX_MS);
}

/** Build the serialisable snapshot of the current session. */
function snapshot() {
  const { groups, activeGroupId } = useEditor.getState();
  const fileGroups = (groups ?? []).map((g) => {
    const tabs = g.tabs
      .filter((t) => t.id?.startsWith('f:') && t.path)
      .map((t) => ({ path: t.path, viewMode: t.viewMode }));
    const active = g.tabs.find((t) => t.id === g.activeId);
    return { tabs, activePath: active?.id?.startsWith('f:') ? active.path : null };
  });
  const agent = useAgent.getState();
  const cliRowId = useExternalAgents.getState().activeSessionId;
  return {
    groups: fileGroups,
    activeGroupIndex: Math.max(0, (groups ?? []).findIndex((g) => g.id === activeGroupId)),
    activeProjectId: useExplorer.getState().activeProjectId ?? null,
    activeTaskId: agent.activeTaskId ?? null,
    chatsByProject: { ...pendingChats, ...chatsByProject },
    cliSession: cliRowId != null && agent.activeProject?.id
      ? { projectId: agent.activeProject.id, rowId: cliRowId }
      : null,
    chatDockOpen: !!useLayout.getState().chatDockOpen,
    // Lets the next launch show the "Restoring…" indicator before settings load.
    restore: !!useSettings.getState().settings?.general?.restore_last_session,
  };
}

/** Start writing the session snapshot to localStorage whenever it changes. */
export function startSessionPersistence() {
  if (persistenceStarted) return;
  persistenceStarted = true;
  let timer = null;
  let last = '';
  const save = () => {
    timer = null;
    try {
      const json = JSON.stringify(snapshot());
      if (json !== last) {
        last = json;
        localStorage.setItem(KEY, json);
      }
    } catch { /* storage full / unavailable — non-fatal */ }
  };
  const schedule = () => {
    if (timer == null) timer = setTimeout(save, SAVE_DEBOUNCE_MS);
  };
  useEditor.subscribe(schedule);
  rememberActiveChat();
  const current = useAgent.getState().activeProject?.id;
  if (current) reopenPendingChat(current).catch(() => {});
  useAgent.subscribe((s, prev) => {
    if (s.activeTaskId !== prev.activeTaskId) {
      rememberActiveChat();
      schedule();
    }
    if (s.activeProject?.id !== prev.activeProject?.id && s.activeProject?.id) {
      reopenPendingChat(s.activeProject.id).catch(() => {});
    }
  });
  useExternalAgents.subscribe((s, prev) => { if (s.activeSessionId !== prev.activeSessionId) schedule(); });
  useExplorer.subscribe((s, prev) => { if (s.activeProjectId !== prev.activeProjectId) schedule(); });
  useLayout.subscribe((s, prev) => { if (s.chatDockOpen !== prev.chatDockOpen) schedule(); });
  useSettings.subscribe((s, prev) => {
    if (s.settings?.general?.restore_last_session !== prev.settings?.general?.restore_last_session) schedule();
  });
  window.addEventListener('beforeunload', save);
  save();
}

/** Write the session snapshot immediately (used right before quitting). */
export function saveSessionNow() {
  if (!persistenceStarted) return;
  try { localStorage.setItem(KEY, JSON.stringify(snapshot())); } catch { /* non-fatal */ }
}

/** True when `path` still exists on disk (missing files are skipped on restore). */
async function exists(path) {
  try {
    return !!(await invoke('stat_path', { path }));
  } catch {
    return false;
  }
}

/** Reopen the previous session's files and chat; call once projects and settings are loaded. */
export async function restoreLastSession(enabled) {
  if (restoreAttempted) return;
  restoreAttempted = true;
  try {
    if (!enabled) return;
    const raw = localStorage.getItem(KEY);
    if (!raw) return;
    const saved = JSON.parse(raw);

    const { projects } = useExplorer.getState();
    if (saved.activeProjectId && projects.some((p) => p.id === saved.activeProjectId)) {
      useExplorer.getState().setActiveProject(saved.activeProjectId);
    }

    const groups = Array.isArray(saved.groups) ? saved.groups.filter((g) => g?.tabs?.length) : [];
    const editor = useEditor.getState();
    let groupId = editor.activeGroupId;
    const groupIds = [];
    for (let i = 0; i < groups.length; i += 1) {
      if (i > 0) groupId = useEditor.getState().splitGroup(groupId);
      groupIds.push(groupId);
      const present = await Promise.all(groups[i].tabs.map((t) => exists(t.path)));
      groups[i].tabs.forEach((t, idx) => {
        if (!present[idx]) return;
        const tabId = useEditor.getState().openFileInGroup(t.path, groupId);
        if (tabId && t.viewMode && t.viewMode !== 'edit') {
          useEditor.getState().setTabViewMode(tabId, groupId, t.viewMode);
        }
      });
      const activePath = groups[i].activePath;
      if (activePath) {
        const tab = useEditor.getState().groups.find((g) => g.id === groupId)?.tabs.find((t) => t.path === activePath);
        if (tab) useEditor.getState().setActiveInGroup(tab.id, groupId);
      }
    }
    const activeGroup = groupIds[Math.min(saved.activeGroupIndex ?? 0, groupIds.length - 1)];
    if (activeGroup) useEditor.getState().setActiveGroup(activeGroup);

    // Chats (the active one included) reopen via pendingChats once the agent
    // store has actually switched to their project — setting activeTaskId now
    // would be wiped by that switch.
    if (saved.chatsByProject && typeof saved.chatsByProject === 'object') {
      for (const [pid, taskId] of Object.entries(saved.chatsByProject)) {
        if (typeof taskId === 'string' && projects.some((p) => p.id === pid)) pendingChats[pid] = taskId;
      }
    }
    if (saved.activeTaskId && saved.activeProjectId) pendingChats[saved.activeProjectId] = saved.activeTaskId;
    const cli = saved.cliSession;
    if (cli?.projectId && cli.rowId != null && cli.projectId === saved.activeProjectId) {
      const rows = await useExternalAgents.getState().loadSessions(cli.projectId);
      const row = rows.find((r) => r.id === cli.rowId);
      if (row) await useExternalAgents.getState().openSession(row);
    }
    if (saved.chatDockOpen) useLayout.getState().openChatDock();
  } catch (e) {
    console.warn('[session] restore failed', e);
  } finally {
    useSessionRestore.setState({ restoring: false });
    startSessionPersistence();
  }
}
