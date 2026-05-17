import { el, icon, iconMulti } from '../../utils/dom.js';
import { agentStore, createTask, setActiveTask, deleteTaskAction, initAgentEvents } from '../../state/agent.js';
import { workspaceStore, addProject, removeProject } from '../../state/workspace.js';
import { openSettings, setCategory } from '../../state/settings.js';
import { focusAgentTerminal, closeTerminal as closeTerminalSession, terminalStore } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';
import { formatRelativeTime } from '../../utils/format-time.js';
import { showConfirmDialog } from '../confirm-dialog.js';

const TERMINAL_STATUSES = new Set(['Completed', 'Failed', 'Cancelled', 'Stopped']);

function formatCost(cost, costKind) {
  if (!cost) return '';
  const usd = cost.estimated_cost_usd || 0;
  const tokens = (cost.total_input_tokens || 0) + (cost.total_output_tokens || 0);
  if (usd > 0.001) return `$${usd.toFixed(3)}${formatCostSuffix(costKind)}`;
  if (tokens > 1000) return `~${(tokens / 1000).toFixed(1)}k`;
  if (tokens > 0) return `~${tokens}`;
  return '';
}

/**
 * P0.8: render the auth-mode suffix that distinguishes a real charge
 * from a subscription-covered estimate. `costKind` is set on the task
 * by the `agent-cost-source` event; absent for native API tasks
 * (always real charges, no suffix needed).
 */
function formatCostSuffix(costKind) {
  switch (costKind) {
    case 'billed_api':             return ' (API)';
    case 'estimated_subscription': return ' (sub estimate)';
    case 'billed_unknown':         return ' (billed)';
    case 'estimated_local':        return ' (estimate)';
    default:                       return '';
  }
}

function makeStatusDot(status, taskId, isStreaming) {
  const dot = el('span', { class: 'agent-task__dot' });
  const pendingPerms = agentStore.getState('permissionRequests')[taskId];
  const effectivelyRunning = status === 'Running' || isStreaming;
  const needsIntervention = effectivelyRunning && pendingPerms && pendingPerms.length > 0;

  if (needsIntervention || status === 'WaitingForInput') {
    dot.classList.add('agent-task__dot--intervention');
  } else if (effectivelyRunning) {
    dot.classList.add('agent-task__dot--running');
  } else if (status === 'Failed') {
    dot.classList.add('agent-task__dot--failed');
  } else if (status === 'Completed') {
    dot.classList.add('agent-task__dot--completed');
  } else {
    dot.classList.add('agent-task__dot--idle');
  }
  return dot;
}

export function createAgentPanel() {
  const panel = el('div', { class: 'agent-panel' });

  const collapsedProjects = new Set();
  const expandedChats = new Set();
  const loadedProjectIds = new Set(); // projects whose tasks have been loaded from DB
  // Cached list of agent-owned terminals (is_agent=true). Refreshed on
  // `terminal-list-changed` events and on mount.
  let agentTerminals = [];

  async function refreshAgentTerminals() {
    try {
      const all = await api.listTerminals();
      agentTerminals = (all || []).filter(t => t.is_agent);

      // Sync the shared terminalStore: drop any sessions whose backend
      // counterpart has vanished (pty exited or the agent killed it),
      // and make sure split/active don't hang onto a dead id.
      const liveIds = new Set((all || []).map(t => t.id));
      const sessions = terminalStore.getState('sessions');
      const prunedSessions = sessions.filter(s => liveIds.has(s.id));
      if (prunedSessions.length !== sessions.length) {
        const activeId = terminalStore.getState('activeSessionId');
        const splitIds = terminalStore.getState('splitSessionIds').filter(id => liveIds.has(id));
        const newActive = liveIds.has(activeId)
          ? activeId
          : (prunedSessions.length > 0 ? prunedSessions[prunedSessions.length - 1].id : null);
        terminalStore.setState({
          sessions: prunedSessions,
          splitSessionIds: splitIds,
          activeSessionId: newActive,
        });
      }

      renderContent();

      // If the terminals modal is open, re-render its list so rows reflect
      // spawns/kills/command updates without needing to close-reopen.
      const openModal = panel.querySelector('.terminals-modal');
      if (openModal && typeof openModal.__rusticListObserver === 'function') {
        openModal.__rusticListObserver();
      }
    } catch (err) {
      console.error('Failed to list terminals:', err);
    }
  }

  function formatElapsed(createdAtMs) {
    if (!createdAtMs) return '';
    const secs = Math.max(0, Math.floor((Date.now() - createdAtMs) / 1000));
    if (secs < 60) return `${secs}s`;
    const mins = Math.floor(secs / 60);
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    return `${hours}h${mins % 60}m`;
  }

  const header = el('div', { class: 'agent-panel__header' });

  // "Agent" static label. The animated bot face that briefly lived here
  // was redundant with the activity-bar's agent icon, so the header is
  // back to a plain text title.
  const titleLabel = el('span', { class: 'agent-panel__title' }, 'Agent');

  // Live-agent counter (plan §B.14). Hidden when zero so the header stays
  // quiet during normal use; appears as a small pill once any harness CLI
  // session is alive in the registry. Total across all projects.
  const liveAgentsBadge = el('span', {
    class: 'agent-panel__live-agents',
    title: 'Live AI agent sessions in this app.',
  });
  liveAgentsBadge.style.display = 'none';

  // Header actions (right side)
  const headerActions = el('div', { class: 'sidebar-header__actions' });

  // Add project (folder-plus)
  const addProjectBtn = el('button', { class: 'sidebar-header__action', title: 'Add Project' });
  addProjectBtn.appendChild(iconMulti([
    'M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z',
    'M12 11v6',
    'M9 14h6',
  ], 13));
  addProjectBtn.addEventListener('click', () => addProject());
  headerActions.appendChild(addProjectBtn);

  // Collapse all project sections
  const collapseAllBtn = el('button', { class: 'sidebar-header__action', title: 'Collapse All' });
  collapseAllBtn.appendChild(iconMulti([
    'M17 11l-5-5-5 5',
    'M17 18l-5-5-5 5',
  ], 13));
  collapseAllBtn.addEventListener('click', () => {
    const projects = workspaceStore.getState('projects');
    for (const p of projects) collapsedProjects.add(p.id);
    expandedChats.clear();
    renderContent();
  });
  headerActions.appendChild(collapseAllBtn);

  // Agent settings (gear)
  const settingsBtn = el('button', { class: 'sidebar-header__action', title: 'Agent settings' });
  settingsBtn.appendChild(iconMulti([
    'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z',
    'M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h0a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z',
  ], 13));
  settingsBtn.addEventListener('click', () => {
    setCategory('agent');
    openSettings();
  });
  headerActions.appendChild(settingsBtn);

  header.appendChild(titleLabel);
  header.appendChild(liveAgentsBadge);
  header.appendChild(headerActions);

  // Live harness CLI session IDs, refreshed every 5 s from the backend.
  // Drives both the header counter (B.14) and the per-project "agents
  // active" banner (B.6). Polling is the simplest path — the registry
  // mutates from many sites (send_message, idle reaper, abort, delete,
  // crash detection) so wiring an event firehose would be invasive.
  let harnessActiveIds = new Set();

  function setsEqual(a, b) {
    if (a.size !== b.size) return false;
    for (const v of a) if (!b.has(v)) return false;
    return true;
  }

  function refreshLiveAgentsBadge() {
    const n = harnessActiveIds.size;
    liveAgentsBadge.textContent = n === 0 ? '' : String(n);
    liveAgentsBadge.style.display = n === 0 ? 'none' : '';
  }
  refreshLiveAgentsBadge();

  async function pollHarnessActive() {
    try {
      const ids = await api.harnessActiveTaskIds();
      const next = new Set(Array.isArray(ids) ? ids : []);
      if (!setsEqual(next, harnessActiveIds)) {
        harnessActiveIds = next;
        refreshLiveAgentsBadge();
        renderContent();
      }
    } catch {
      // Older build without the command — fall through, panel still works.
    }
  }
  pollHarnessActive();
  setInterval(pollHarnessActive, 5000);

  const content = el('div', { class: 'agent-panel__content' });


  function openHistoryModal(project) {
    panel.querySelector('.history-modal')?.remove();

    const modal = el('div', { class: 'history-modal' });

    // Header
    const modalHeader = el('div', { class: 'history-modal__header' });
    modalHeader.appendChild(el('span', { class: 'history-modal__title' }, `History — ${project.name}`));
    const closeBtn = el('button', { class: 'history-modal__close', title: 'Close' });
    closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
    closeBtn.addEventListener('click', () => modal.remove());
    modalHeader.appendChild(closeBtn);
    modal.appendChild(modalHeader);

    // Actions row
    const actionsRow = el('div', { class: 'history-modal__actions' });
    const clearAllBtn = el('button', { class: 'history-modal__clear-all' }, 'Clear All');
    clearAllBtn.addEventListener('click', async () => {
      clearAllBtn.disabled = true;
      try {
        await api.deleteTasksForProject(project.id);
        const tasks = { ...agentStore.getState('tasks') };
        for (const id of Object.keys(tasks)) {
          if (tasks[id].project_id === project.id || tasks[id].projectId === project.id) {
            delete tasks[id];
          }
        }
        agentStore.setState({ tasks });
        modal.remove();
      } catch (err) {
        console.error('Failed to clear history:', err);
        clearAllBtn.disabled = false;
      }
    });
    actionsRow.appendChild(clearAllBtn);
    modal.appendChild(actionsRow);

    // Task list
    const list = el('div', { class: 'history-modal__list' });

    function renderList() {
      list.innerHTML = '';
      const tasks = agentStore.getState('tasks');
      const histTasks = Object.values(tasks)
        .filter(t => (t.project_id === project.id || t.projectId === project.id) && TERMINAL_STATUSES.has(t.status))
        .sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || ''));

      if (histTasks.length === 0) {
        list.appendChild(el('div', { class: 'history-modal__empty' }, 'No history yet'));
        return;
      }

      for (const task of histTasks) {
        const row = el('div', { class: 'history-modal__item' });

        const statusIcon = el('span', { class: 'history-modal__status' });
        if (task.status === 'Failed') {
          statusIcon.appendChild(icon('M18 6L6 18M6 6l12 12', 11));
          statusIcon.style.color = 'var(--bright-red)';
        } else if (task.status === 'Cancelled' || task.status === 'Stopped') {
          statusIcon.appendChild(icon('M18 6L6 18M6 6l12 12', 11));
          statusIcon.style.color = 'var(--fg4)';
        } else {
          statusIcon.appendChild(icon('M5 12l5 5L20 7', 11));
          statusIcon.style.color = 'var(--bright-green)';
        }
        row.appendChild(statusIcon);

        const titleEl = el('span', { class: 'history-modal__item-title' }, task.title || 'Untitled');
        row.appendChild(titleEl);

        const deleteBtn = el('button', { class: 'history-modal__delete', title: 'Delete' });
        deleteBtn.appendChild(icon('M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6', 12));
        deleteBtn.addEventListener('click', async (e) => {
          e.stopPropagation();
          deleteBtn.disabled = true;
          try {
            await deleteTaskAction(task.id);
            renderList();
          } catch {
            deleteBtn.disabled = false;
          }
        });
        row.appendChild(deleteBtn);

        row.addEventListener('click', () => {
          agentStore.setState({ activeTaskId: task.id });
          modal.remove();
        });

        list.appendChild(row);
      }
    }

    renderList();

    // Re-render list when tasks change; clean up subscription when modal is closed
    const unsub = agentStore.subscribe('tasks', renderList);
    closeBtn.addEventListener('click', unsub, { once: true });
    clearAllBtn.addEventListener('click', unsub, { once: true });

    modal.appendChild(list);
    panel.appendChild(modal);
  }


  const VISIBLE_CHAT_LIMIT = 5;

  function buildProjectSection(project, tasks, activeTaskId) {
    const projectTasks = Object.values(tasks)
      .filter(t => t.project_id === project.id || t.projectId === project.id)
      .sort((a, b) => {
        if (a.status === 'Running' && b.status !== 'Running') return -1;
        if (b.status === 'Running' && a.status !== 'Running') return 1;
        const aMs = new Date(a.updated_at || a.updatedAt || a.created_at || a.createdAt || 0).getTime();
        const bMs = new Date(b.updated_at || b.updatedAt || b.created_at || b.createdAt || 0).getTime();
        return bMs - aMs;
      });

    const section = el('div', { class: 'agent-project' });
    const isCollapsed = collapsedProjects.has(project.id);

    const runningCount = projectTasks.filter(t => t.status === 'Running').length;

    // Project header row
    const projHeader = el('div', { class: 'agent-project__header' });

    // Clickable left section (caret + name + count) — mirrors file explorer.
    const headerLeft = el('div', {
      class: 'agent-project__header-left',
      title: isCollapsed ? 'Expand' : 'Collapse',
      onClick: () => {
        if (collapsedProjects.has(project.id)) collapsedProjects.delete(project.id);
        else collapsedProjects.add(project.id);
        renderContent();
      },
    });

    const caretIcon = icon(
      isCollapsed ? 'M9 18l6-6-6-6' : 'M6 9l6 6 6-6',
      12,
    );
    const caret = el('span', { class: 'agent-project__caret' }, caretIcon);
    headerLeft.appendChild(caret);

    headerLeft.appendChild(el('span', { class: 'agent-project__name' }, project.name));

    if (runningCount > 0) {
      headerLeft.appendChild(el('span', { class: 'agent-project__count' }, String(runningCount)));
    }

    projHeader.appendChild(headerLeft);

    const actionGroup = el('div', { class: 'agent-project__actions' });

    const newBtn = el('button', { class: 'agent-project__new', title: 'New Task' });
    newBtn.appendChild(icon('M12 5v14M5 12h14', 12));
    newBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      // Reuse an existing empty task for this project instead of creating a duplicate
      const tasks = agentStore.getState('tasks');
      const emptyTask = Object.values(tasks).find(t =>
        (t.project_id === project.id || t.projectId === project.id) &&
        (!t.messages || t.messages.length === 0) &&
        (t.title === 'New Task' || !t.title)
      );
      if (emptyTask) {
        setActiveTask(emptyTask.id);
        return;
      }
      createTask(project.id, project.name, project.root_path, 'New Task');
    });
    actionGroup.appendChild(newBtn);

    const projectAgentTerminals = agentTerminals.filter(t => isTerminalForProject(t, project));
    const terminalsBtn = el('button', {
      class: 'agent-project__new',
      title: `Agent Terminals (${projectAgentTerminals.length})`,
    });
    terminalsBtn.appendChild(icon('M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2z', 12));
    if (projectAgentTerminals.length > 0) {
      terminalsBtn.appendChild(el('span', { class: 'agent-project__new-badge' }, String(projectAgentTerminals.length)));
    }
    terminalsBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      openTerminalsModal(project);
    });
    actionGroup.appendChild(terminalsBtn);

    const removeBtn = el('button', { class: 'agent-project__new', title: 'Remove Project' });
    removeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
    removeBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      const ok = await showConfirmDialog(
        'Remove project?',
        `${project.name || project.root_path} will be removed from the workspace. ` +
        `Files on disk are not deleted, but tasks and terminal ` +
        `sessions tied to this project will be cleared.`,
        { confirmLabel: 'Remove', cancelLabel: 'Keep', danger: true },
      );
      if (ok) removeProject(project.id);
    });
    actionGroup.appendChild(removeBtn);

    projHeader.appendChild(actionGroup);

    section.appendChild(projHeader);

    // Concurrency-cap warning (plan §B.6). Soft threshold: ≥ 4 live harness
    // CLI sessions in a single project means simultaneous tool batches
    // could clobber each other on shared files. Always rendered (even when
    // the project section is collapsed) so the user notices.
    const projectTaskIdSet = new Set(projectTasks.map((t) => t.id));
    const projectHarnessCount = (() => {
      let n = 0;
      for (const id of harnessActiveIds) if (projectTaskIdSet.has(id)) n++;
      return n;
    })();
    if (projectHarnessCount >= 4) {
      const warn = el('div', { class: 'agent-project__cap-warning' });
      // Triangle with exclamation — Heroicons "exclamation-triangle".
      warn.appendChild(icon('M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0zM12 9v4M12 17h.01', 13));
      warn.appendChild(el('span', {},
        `${projectHarnessCount} agents active in this project — file conflicts possible.`));
      section.appendChild(warn);
    }

    if (!isCollapsed) {
      const taskList = el('div', { class: 'agent-project__tasks' });
      const isExpanded = expandedChats.has(project.id);
      const visibleTasks = isExpanded ? projectTasks : projectTasks.slice(0, VISIBLE_CHAT_LIMIT);

      for (const task of visibleTasks) {
        taskList.appendChild(buildTaskRow(task, activeTaskId, false));
      }

      const hiddenCount = projectTasks.length - VISIBLE_CHAT_LIMIT;
      if (!isExpanded && hiddenCount > 0) {
        const expandBtn = el('button', { class: 'agent-expand-btn' });
        expandBtn.textContent = `+ ${hiddenCount} more`;
        expandBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          expandedChats.add(project.id);
          renderContent();
        });
        taskList.appendChild(expandBtn);
      } else if (isExpanded && projectTasks.length > VISIBLE_CHAT_LIMIT) {
        const collapseBtn = el('button', { class: 'agent-expand-btn agent-expand-btn--collapse' });
        collapseBtn.textContent = 'Show less';
        collapseBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          expandedChats.delete(project.id);
          renderContent();
        });
        taskList.appendChild(collapseBtn);
      }

      section.appendChild(taskList);
    }


    return section;
  }

  function isTerminalForProject(term, project) {
    // Match terminals that live inside the project root. Fall back to
    // showing all agent terminals if either side is missing a path.
    if (!term.cwd || !project.root_path) return true;
    // Windows paths are case-insensitive; normalize separators.
    const norm = (p) => String(p).replace(/\\/g, '/').toLowerCase().replace(/\/+$/, '');
    const root = norm(project.root_path);
    const cwd = norm(term.cwd);
    return cwd === root || cwd.startsWith(root + '/');
  }

  function openTerminalsModal(project) {
    panel.querySelector('.terminals-modal')?.remove();

    const modal = el('div', { class: 'terminals-modal' });

    // Header — just the project name, with the close button on the right.
    const modalHeader = el('div', { class: 'terminals-modal__header' });
    modalHeader.appendChild(el('span', { class: 'terminals-modal__title' }, project.name));
    const closeBtn = el('button', { class: 'terminals-modal__close', title: 'Close' });
    closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
    closeBtn.addEventListener('click', () => modal.remove());
    modalHeader.appendChild(closeBtn);
    modal.appendChild(modalHeader);

    // List
    const list = el('div', { class: 'terminals-modal__list' });

    function renderList() {
      list.innerHTML = '';
      const terms = agentTerminals.filter(t => isTerminalForProject(t, project));
      if (terms.length === 0) {
        list.appendChild(el('div', { class: 'terminals-modal__empty' }, 'No active agent terminals'));
        return;
      }
      for (const term of terms) {
        list.appendChild(buildTerminalModalRow(term, project));
      }
    }

    renderList();

    // Re-render when the panel's `refreshAgentTerminals` fires (on
    // `terminal-list-changed` events from the backend).
    modal.__rusticListObserver = renderList;

    modal.appendChild(list);
    panel.appendChild(modal);
  }

  function buildTerminalModalRow(term, project) {
    const row = el('div', { class: 'terminals-modal__row' });

    const names = el('div', { class: 'terminals-modal__names' });
    names.appendChild(el('div', { class: 'terminals-modal__primary' }, 'agent terminal'));
    names.appendChild(el('div', { class: 'terminals-modal__secondary' }, project.name));
    row.appendChild(names);

    // Actions: Open + Delete
    const actions = el('div', { class: 'terminals-modal__row-actions' });

    const openBtn = el('button', { class: 'terminals-modal__icon-btn', title: 'Open in terminal panel' });
    // "open in external" icon (arrow out of box)
    openBtn.appendChild(icon('M14 3h7v7M10 14L21 3M19 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h6', 13));
    openBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      focusAgentTerminal({ id: term.id, label: term.label, cwd: term.cwd, is_agent: true });
      // Modal stays open intentionally so the user can open more.
    });
    actions.appendChild(openBtn);

    const delBtn = el('button', { class: 'terminals-modal__icon-btn terminals-modal__icon-btn--danger', title: 'Kill terminal' });
    // trash icon
    delBtn.appendChild(icon('M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6', 13));
    delBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      delBtn.disabled = true;
      try {
        // Use the shared store helper: removes from terminalStore (bottom panel
        // tabs) AND calls api.closeTerminal, so the process dies whether or
        // not the user had it opened in the bottom panel.
        await closeTerminalSession(term.id);
      } catch (err) {
        console.error('Failed to close terminal:', err);
        delBtn.disabled = false;
      }
    });
    actions.appendChild(delBtn);

    row.appendChild(actions);
    return row;
  }

  function buildTaskRow(task, activeTaskId, isHistory) {
    const taskEl = el('div', {
      class: `agent-task ${task.id === activeTaskId ? 'agent-task--active' : ''}`,
    });

    taskEl.appendChild(makeStatusDot(task.status, task.id, task.isStreaming));

    // Relative-time label sitting between the status dot and the title —
    // mirrors the welcome-screen history layout. Reads updated_at first
    // (activity recency) so a long-idle task falls below a freshly-used
    // one that was created earlier; camelCase checked too for payloads
    // that came through older serializer paths.
    const ts = task.updated_at || task.updatedAt || task.created_at || task.createdAt || '';
    const rel = formatRelativeTime(ts);
    if (rel) {
      taskEl.appendChild(el(
        'span',
        { class: 'agent-task__time', title: ts },
        rel,
      ));
    }

    const titleEl = el('span', { class: 'agent-task__title' }, task.title);
    taskEl.appendChild(titleEl);

    // Show stop button for running tasks
    if (!isHistory && task.status === 'Running') {
      const stopBtn = el('button', { class: 'agent-task__stop', title: 'Stop Task' });
      stopBtn.appendChild(icon('M6 6h12v12H6z', 10));
      stopBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        api.abortTask(task.id).catch(err => console.error('Failed to abort task:', err));
      });
      taskEl.appendChild(stopBtn);
    }

    const deleteBtn = el('button', { class: 'agent-task__delete', title: 'Delete Task' });
    deleteBtn.appendChild(icon('M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', 10));
    deleteBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      deleteTaskAction(task.id);
    });
    taskEl.appendChild(deleteBtn);

    taskEl.addEventListener('click', () => setActiveTask(task.id));
    return taskEl;
  }

  function formatDate(isoStr) {
    if (!isoStr) return '';
    try {
      const d = new Date(isoStr);
      const now = new Date();
      const diffDays = Math.floor((now - d) / 86400000);
      if (diffDays === 0) return 'Today';
      if (diffDays === 1) return 'Yesterday';
      if (diffDays < 7) return `${diffDays}d ago`;
      return d.toLocaleDateString();
    } catch {
      return '';
    }
  }


  async function loadProjectTasks(projectId) {
    if (loadedProjectIds.has(projectId)) return;
    loadedProjectIds.add(projectId);
    try {
      const infos = await api.listTasks(projectId);
      if (!infos?.length) return;
      const tasks = { ...agentStore.getState('tasks') };
      let changed = false;
      const newIds = [];
      const TERMINAL = new Set(['Completed', 'Failed', 'Cancelled', 'Stopped']);
      for (const info of infos) {
        if (!tasks[info.id]) {
          tasks[info.id] = { ...info, messages: [], isStreaming: false };
          newIds.push(info.id);
          changed = true;
        } else if (TERMINAL.has(info.status)) {
          // Backend reports the task is no longer running. Force-clear any
          // stale `isStreaming: true` left over from before a backend restart
          // — without this, the chat-view's "Agent is running..." placeholder
          // (and the red Stop button) keep showing because the in-memory
          // task still has `isStreaming: true` from an event emitted by the
          // previous process that never paired with a Cancelled/Completed
          // status event.
          const prev = tasks[info.id];
          if (prev.status !== info.status || prev.isStreaming) {
            tasks[info.id] = { ...prev, status: info.status, isStreaming: false };
            changed = true;
          }
        }
      }
      if (changed) agentStore.setState({ tasks });
      // Fetch cost data for newly loaded tasks (async, non-blocking)
      for (const id of newIds) {
        api.getTaskCost(id).then(cost => {
          if (!cost) return;
          const t = { ...agentStore.getState('tasks') };
          if (t[id]) {
            t[id] = { ...t[id], cost };
            agentStore.setState({ tasks: t });
          }
        }).catch(() => {});
      }
    } catch {}
  }

  function renderContent() {
    content.innerHTML = '';
    const wrap = el('div', { class: 'agent-tab-content' });
    const projects = workspaceStore.getState('projects');
    const tasks = agentStore.getState('tasks');
    const activeTaskId = agentStore.getState('activeTaskId');

    if (projects.length === 0) {
      wrap.appendChild(el('div', { class: 'panel-placeholder' }, 'No projects open'));
    } else {
      for (const project of projects) {
        loadProjectTasks(project.id); // load from DB on first encounter (async, triggers re-render)
        wrap.appendChild(buildProjectSection(project, tasks, activeTaskId));
      }
    }

    content.appendChild(wrap);
  }

  // Subscribe to store changes
  agentStore.subscribe('tasks', () => renderContent());
  agentStore.subscribe('activeTaskId', () => renderContent());
  agentStore.subscribe('permissionRequests', () => renderContent());
  workspaceStore.subscribe('projects', () => renderContent());

  panel.appendChild(header);
  panel.appendChild(content);

  renderContent();
  initAgentEvents();

  // Live-refresh the agent-terminals list whenever the backend reports a change
  // (create_terminal, close_terminal, or pty exit).
  api.onTerminalListChanged(() => {
    refreshAgentTerminals();
  }).catch(err => console.error('Failed to subscribe to terminal-list-changed:', err));
  refreshAgentTerminals();

  // Re-render once a minute so elapsed-time labels stay fresh while panel is mounted.
  setInterval(() => {
    if (agentTerminals.length > 0) renderContent();
  }, 60000);

  return panel;
}
