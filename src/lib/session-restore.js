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
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useSettings } from '@/state/settings';

const KEY = 'rustic.session.v1';
const SAVE_DEBOUNCE_MS = 400;
const RESTORE_INDICATOR_MAX_MS = 20000;

let persistenceStarted = false;
let restoreAttempted = false;

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
  return {
    groups: fileGroups,
    activeGroupIndex: Math.max(0, (groups ?? []).findIndex((g) => g.id === activeGroupId)),
    activeProjectId: useExplorer.getState().activeProjectId ?? null,
    activeTaskId: agent.activeTaskId ?? null,
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
  useAgent.subscribe((s, prev) => { if (s.activeTaskId !== prev.activeTaskId) schedule(); });
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

    if (saved.activeTaskId) useAgent.getState().setActiveTask(saved.activeTaskId);
    if (saved.chatDockOpen) useLayout.getState().openChatDock();
  } catch (e) {
    console.warn('[session] restore failed', e);
  } finally {
    useSessionRestore.setState({ restoring: false });
    startSessionPersistence();
  }
}
