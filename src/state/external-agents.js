import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { useTerminal, terminalHadSubmit, onTerminalFirstSubmit } from '@/state/terminal';
import { useAgent } from '@/state/agent';
import { useLayout } from '@/state/layout';

const SESSIONS_CHANGED_EVENT = 'external-agent-sessions-changed';

// A CLI agent session is a *chat*, not a terminal: the backend spawns its PTY,
// but it is immediately hidden from the bottom terminal panel and the only
// place it surfaces is the agent chat panel (see CliChatView). Hiding is keyed
// by session id, so a later refreshSessions can't un-hide it.
async function adoptPty(sessionId) {
  const terminal = useTerminal.getState();
  // Hide first, then refresh: `hiddenSessionIds` persists across refreshes, so
  // marking it hidden before the session list repopulates guarantees the bottom
  // panel never counts (or shows) this PTY, even under the concurrent refresh
  // the backend's `terminal-list-changed` event triggers.
  terminal.hideTerminal(sessionId);
  await terminal.refreshSessions();
}

export const useExternalAgents = create((set, get) => ({
  // `[{ agent, label, path, version, shadowed, latestVersion }]` for every CLI
  // found on PATH, each resolved to its newest installed copy.
  installed: [],
  detected: false,
  // projectId -> rows, newest first.
  sessionsByProject: {},
  loadedProjects: {},
  // Which project+agent is currently being launched, so exactly one icon spins.
  // Project-scoped: the launcher row is rendered for every project, so a bare
  // agent kind here would spin the same icon in all of them at once.
  launching: null,
  // session row id -> live PTY session id, for the sessions started in this app
  // run. A row with no entry is "stopped" and has to be resumed to be used.
  ptyBySession: {},
  // Row id of the CLI chat the agent panel is currently showing (null = the
  // normal Rustic chat).
  activeSessionId: null,
  // A freshly launched session the user hasn't sent anything in yet. It exists
  // as a PTY and a row (both are needed for the CLI to run at all), but it's a
  // placeholder: hidden from the chat list, and thrown away if the user leaves
  // without a first turn — the same "no message, no chat" rule Rustic's own
  // chats follow.
  draftRowId: null,

  detect: async ({ force = false } = {}) => {
    if (!force && get().detected) return get().installed;
    try {
      const installed = await invoke('detect_external_agents');
      set({ installed, detected: true });
      return installed;
    } catch {
      set({ detected: true });
      return [];
    }
  },

  loadSessions: async (projectId, { force = false } = {}) => {
    if (!projectId) return [];
    if (!force && get().loadedProjects[projectId]) {
      return get().sessionsByProject[projectId] ?? [];
    }
    try {
      const rows = await invoke('list_external_agent_sessions', {
        projectId,
        agent: null,
        limit: 100,
      });
      set((s) => ({
        sessionsByProject: { ...s.sessionsByProject, [projectId]: rows },
        loadedProjects: { ...s.loadedProjects, [projectId]: true },
      }));
      get().graduateDraft();
      return rows;
    } catch {
      return [];
    }
  },

  // Refresh every project whose sessions have already been fetched.
  refreshLoaded: async () => {
    const ids = Object.keys(get().loadedProjects);
    await Promise.all(ids.map((id) => get().loadSessions(id, { force: true })));
  },

  // A placeholder becomes a real chat as soon as it has had a first turn.
  graduateDraft: () => {
    const rowId = get().draftRowId;
    if (!rowId) return;
    const row = findRow(get(), rowId);
    if (!row || isCommitted(get(), row)) set({ draftRowId: null });
  },

  // Drop a placeholder the user never sent anything in: kill the process, the
  // row, and the CLI's own (empty) transcript. A session that did get a turn is
  // kept and simply stops being a draft.
  discardDraft: async () => {
    const rowId = get().draftRowId;
    if (!rowId) return;
    set({ draftRowId: null });
    const row = findRow(get(), rowId);
    if (!row || isCommitted(get(), row)) return;
    try {
      await get().forget(row);
    } catch {
      /* the row is already gone, or the backend refused — nothing to recover */
    }
  },

  spawn: async (agent, projectId, prompt = null) => {
    await get().discardDraft();
    set({ launching: launchKey(projectId, agent) });
    try {
      const res = await invoke('spawn_external_agent', {
        agent,
        projectId,
        prompt,
        permissionMode: externalAgentPermissionMode(),
      });
      await adoptPty(res.sessionId);
      set((s) => ({
        ptyBySession: { ...s.ptyBySession, [res.rowId]: res.sessionId },
        // A spawn with a prompt already has its first turn, so it isn't a draft.
        draftRowId: prompt?.trim() ? s.draftRowId : res.rowId,
      }));
      await get().loadSessions(projectId, { force: true });
      get().openSessionView(res.rowId);
      return res;
    } finally {
      set({ launching: null });
    }
  },

  // Remove a row from local state everywhere: the project list, the active
  // view, the draft slot and the pty map. Used both by an explicit delete and
  // to self-heal a stale row the backend no longer has.
  _dropRow: (row) => {
    set((s) => {
      const ptyBySession = { ...s.ptyBySession };
      delete ptyBySession[row.id];
      return {
        sessionsByProject: {
          ...s.sessionsByProject,
          [row.project_id]: (s.sessionsByProject[row.project_id] ?? []).filter(
            (r) => r.id !== row.id,
          ),
        },
        ptyBySession,
        activeSessionId: s.activeSessionId === row.id ? null : s.activeSessionId,
        draftRowId: s.draftRowId === row.id ? null : s.draftRowId,
      };
    });
  },

  resume: async (row) => {
    set({ launching: launchKey(row.project_id, row.agent) });
    try {
      let res;
      try {
        res = await invoke('resume_external_agent', {
          sessionRowId: row.id,
          permissionMode: externalAgentPermissionMode(),
        });
      } catch (e) {
        // The conversation is gone from the backend (e.g. an abandoned
        // placeholder that never had a first turn, orphaned by a reload). Drop
        // the stale row instead of leaving an unusable "session not found" entry.
        if (String(e).includes('not found')) {
          get()._dropRow(row);
          toast.error('This CLI session no longer exists — removed it from the list.');
          return null;
        }
        throw e;
      }
      await adoptPty(res.sessionId);
      set((s) => ({ ptyBySession: { ...s.ptyBySession, [row.id]: res.sessionId } }));
      await get().loadSessions(row.project_id, { force: true });
      get().openSessionView(row.id);
      return res;
    } finally {
      set({ launching: null });
    }
  },

  // Show a CLI session in the agent panel, resuming it first when its process is
  // gone (app restart, or the user stopped it) and the CLI can resume it.
  openSession: async (row) => {
    if (get().draftRowId && get().draftRowId !== row.id) await get().discardDraft();
    if (get().ptyBySession[row.id] != null) {
      get().openSessionView(row.id);
      return;
    }
    if (!row.external_session_id) return;
    await get().resume(row);
  },

  openSessionView: (rowId) => {
    useLayout.getState().openChatDock();
    set({ activeSessionId: rowId });
  },

  closeSessionView: () => {
    const { activeSessionId, draftRowId } = get();
    if (activeSessionId == null) return;
    set({ activeSessionId: null });
    // Leaving a placeholder without having sent anything throws it away.
    if (draftRowId === activeSessionId) get().discardDraft();
  },

  // Stop the CLI process but keep the conversation — resumable later through the
  // tool's own resume flag.
  stop: async (row) => {
    const ptyId = get().ptyBySession[row.id];
    set((s) => {
      const ptyBySession = { ...s.ptyBySession };
      delete ptyBySession[row.id];
      return { ptyBySession };
    });
    if (ptyId != null) await useTerminal.getState().closeTerminal(ptyId);
    // The user may have updated the CLI from inside that session, so the version
    // shown in the launcher tooltip is now suspect. (What gets *launched* is
    // resolved fresh in the backend either way.)
    get().detect({ force: true }).catch(() => {});
  },

  // Delete the conversation for good: kill the process, drop Rustic's row, and
  // (best effort) the CLI's own transcript, so it also disappears from that
  // tool's native session picker.
  forget: async (row) => {
    await get().stop(row);
    try {
      await invoke('delete_external_agent_session', {
        sessionRowId: row.id,
        purgeTranscript: true,
      });
    } catch (e) {
      // Already gone from the DB (or the backend refused) — still remove it from
      // the UI so a stale row can never get stuck as "session not found".
      console.error('[external-agents] delete_external_agent_session failed', e);
    }
    get()._dropRow(row);
  },
}));

/** Identity of an in-flight launch: the same agent can be starting in several projects. */
export function launchKey(projectId, agent) {
  return `${projectId}:${agent}`;
}

// Map Rustic's own permission model onto the autonomy level an external CLI is
// launched with. Chat → read-only; Edit → edit (each CLI's interactive
// default); Auto → auto-apply edits; Auto + "grant access to all files" → full
// permission bypass. The backend maps this string to each CLI's own flags.
function externalAgentPermissionMode() {
  const s = useAgent.getState();
  if (s.permissionLevel === 'Chat') return 'read-only';
  if (s.permissionLevel === 'FullAuto') return s.sensitiveAccess ? 'bypass' : 'auto';
  return 'edit';
}

/** The row with this id, from whichever project has it loaded. */
function findRow(state, rowId) {
  for (const rows of Object.values(state.sessionsByProject)) {
    const hit = rows?.find((r) => r.id === rowId);
    if (hit) return hit;
  }
  return null;
}

// Whether a session has had a real first turn. The title (set from the CLI's
// first-prompt hook) is the primary signal; a submitted line in its PTY is the
// fallback for a machine where those hooks can't run, so a conversation the user
// did start is never silently thrown away.
function isCommitted(state, row) {
  if (row.title) return true;
  const ptyId = state.ptyBySession[row.id];
  return ptyId != null && terminalHadSubmit(ptyId);
}

/**
 * The row backing the CLI chat currently open in the agent panel, or null.
 * Rows live per project, so this scans the loaded projects instead of copying
 * the row into state (where it would go stale as the title arrives).
 */
export function selectActiveCliSession(s) {
  if (!s.activeSessionId) return null;
  return findRow(s, s.activeSessionId);
}

let unlisten = null;
let unsubTask = null;
let unsubSubmit = null;

// Subscribe once to the backend's session-changed event.
export function initExternalAgents() {
  if (unlisten) return;
  unlisten = listen(SESSIONS_CHANGED_EVENT, () => {
    useExternalAgents.getState().refreshLoaded().catch(() => {});
  }).catch(() => null);
  // Sending the first line is what turns a placeholder into a real chat, so it
  // has to show up in the list right then — waiting for the tool's hook would
  // hide a conversation the user is already having (and hides it forever on a
  // machine where the hooks can't run).
  if (!unsubSubmit) {
    unsubSubmit = onTerminalFirstSubmit(() => {
      useExternalAgents.getState().graduateDraft();
    });
  }
  // Picking a normal Rustic chat anywhere in the app pops the panel back out of
  // the CLI view — cheaper and harder to forget than clearing the selection at
  // every activeTaskId write site.
  if (!unsubTask) {
    let prev = useAgent.getState().activeTaskId;
    unsubTask = useAgent.subscribe((s) => {
      if (s.activeTaskId === prev) return;
      prev = s.activeTaskId;
      if (s.activeTaskId) useExternalAgents.getState().closeSessionView();
    });
  }
}
