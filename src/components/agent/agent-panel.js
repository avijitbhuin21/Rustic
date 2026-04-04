import { el, icon } from '../../utils/dom.js';
import { agentStore, createTask, setActiveTask, deleteTaskAction, initAgentEvents } from '../../state/agent.js';
import { workspaceStore } from '../../state/workspace.js';
import { openSettings, setCategory } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';

const TERMINAL_STATUSES = new Set(['Completed', 'Failed', 'Cancelled', 'TurnLimitReached', 'Stopped']);

function formatCost(cost) {
  if (!cost) return '';
  const usd = cost.estimated_cost_usd || 0;
  const tokens = (cost.total_input_tokens || 0) + (cost.total_output_tokens || 0);
  if (usd > 0.001) return `$${usd.toFixed(3)}`;
  if (tokens > 1000) return `~${(tokens / 1000).toFixed(1)}k`;
  if (tokens > 0) return `~${tokens}`;
  return '';
}

function makeStatusDot(status) {
  const dot = el('span', { class: 'agent-task__dot' });
  if (status === 'Running') {
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

  // ── Local state ───────────────────────────────────────────
  const collapsedProjects = new Set();
  const expandedHistory = new Set();
  const expandedTerminals = new Set();
  const loadedProjectIds = new Set(); // projects whose tasks have been loaded from DB

  // ── Header ────────────────────────────────────────────────
  const header = el('div', { class: 'agent-panel__header' });

  // "Agent" static label
  const titleLabel = el('span', { class: 'agent-panel__title' }, 'Agent');

  // Header actions (right side)
  const headerActions = el('div', { class: 'sidebar-header__actions' });

  function makeHeaderBtn(title, svgPath) {
    const btn = el('button', { class: 'sidebar-header__action', title });
    btn.appendChild(icon(svgPath, 13));
    btn.addEventListener('click', () => {
      setCategory('agent');
      openSettings();
    });
    return btn;
  }

  headerActions.appendChild(makeHeaderBtn(
    'Configure Providers',
    'M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4'
  ));
  headerActions.appendChild(makeHeaderBtn(
    'MCP Servers',
    'M5 12H3m16 0h-2M12 5V3m0 16v-2m-4.95-1.05-1.414 1.414M18.364 5.636l-1.414 1.414M18.364 18.364l-1.414-1.414M6.05 6.05 4.636 4.636M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z'
  ));
  headerActions.appendChild(makeHeaderBtn(
    'Skills',
    'M13 10V3L4 14h7v7l9-11h-7z'
  ));
  headerActions.appendChild(makeHeaderBtn(
    'Workflows',
    'M6 3v12M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM15 6h-3a6 6 0 0 0-6 6v3'
  ));

  header.appendChild(titleLabel);
  header.appendChild(headerActions);

  // ── Content area ─────────────────────────────────────────
  const content = el('div', { class: 'agent-panel__content' });

  // ── Project context menu ──────────────────────────────────

  function openProjectMenu(anchor, project) {
    document.querySelector('.agent-project-menu')?.remove();

    const menu = el('div', { class: 'agent-project-menu' });

    function addItem(label, svgPath, active, onClick) {
      const item = el('button', { class: `agent-project-menu__item${active ? ' agent-project-menu__item--active' : ''}` });
      const ico = icon(svgPath, 13);
      item.appendChild(ico);
      item.appendChild(document.createTextNode(label));
      item.addEventListener('click', (e) => {
        e.stopPropagation();
        menu.remove();
        onClick();
      });
      menu.appendChild(item);
    }

    addItem(
      'Active Terminals',
      'M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2z',
      expandedTerminals.has(project.id),
      () => {
        if (expandedTerminals.has(project.id)) expandedTerminals.delete(project.id);
        else expandedTerminals.add(project.id);
        renderContent();
      }
    );

    addItem(
      'Task History',
      'M12 8v4l3 3m6-3a9 9 0 1 1-18 0 9 9 0 0 1 18 0',
      false,
      () => openHistoryModal(project)
    );

    addItem(
      'Open Project Memory',
      'M9 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2M9 5a2 2 0 0 0 2 2h2a2 2 0 0 0 2-2M9 5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2',
      false,
      async () => {
        try {
          const allProjects = workspaceStore.getState('projects');
          const proj = allProjects.find(p => p.id === project.id);
          if (!proj?.root_path) return;
          await api.getMemory(project.id);
          await api.openFile(`${proj.root_path}/.rustic/memory.md`);
        } catch (err) {
          console.error('Failed to open memory file:', err);
        }
      }
    );

    const rect = anchor.getBoundingClientRect();
    menu.style.top = `${rect.bottom + 4}px`;
    menu.style.right = `${window.innerWidth - rect.right}px`;
    document.body.appendChild(menu);

    const close = (e) => {
      if (!menu.contains(e.target) && e.target !== anchor) {
        menu.remove();
        document.removeEventListener('click', close, true);
      }
    };
    setTimeout(() => document.addEventListener('click', close, true), 0);
  }

  // ── History modal ─────────────────────────────────────────

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

  // ── Project sections ──────────────────────────────────────

  function buildProjectSection(project, tasks, activeTaskId) {
    // Show all tasks for this project (up to 5 latest), not just non-terminal
    const projectTasks = Object.values(tasks)
      .filter(t => t.project_id === project.id || t.projectId === project.id)
      .sort((a, b) => {
        // Running tasks first, then by most recent
        if (a.status === 'Running' && b.status !== 'Running') return -1;
        if (b.status === 'Running' && a.status !== 'Running') return 1;
        const aTime = a.updated_at || a.created_at || '';
        const bTime = b.updated_at || b.created_at || '';
        return bTime.localeCompare(aTime);
      })
      .slice(0, 5);

    const section = el('div', { class: 'agent-project' });
    const isCollapsed = collapsedProjects.has(project.id);

    const runningCount = projectTasks.filter(t => t.status === 'Running').length;

    // Project header row
    const projHeader = el('div', { class: 'agent-project__header' });

    // Toggle arrow
    const toggleBtn = el('button', { class: 'agent-project__toggle', title: isCollapsed ? 'Expand' : 'Collapse' });
    toggleBtn.innerHTML = isCollapsed
      ? '<svg width="10" height="10" viewBox="0 0 10 10"><path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>'
      : '<svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 3l3 4 3-4" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    toggleBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      if (collapsedProjects.has(project.id)) collapsedProjects.delete(project.id);
      else collapsedProjects.add(project.id);
      renderContent();
    });
    projHeader.appendChild(toggleBtn);

    projHeader.appendChild(el('span', { class: 'agent-project__name' }, project.name));

    if (runningCount > 0) {
      projHeader.appendChild(el('span', { class: 'agent-project__count' }, String(runningCount)));
    }

    // Action group (+ and ⋮ side by side)
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

    const menuBtn = el('button', { class: 'agent-project__menu-btn', title: 'More options' });
    menuBtn.innerHTML = '<svg width="13" height="13" viewBox="0 0 13 13" fill="currentColor"><circle cx="6.5" cy="2.5" r="1.1"/><circle cx="6.5" cy="6.5" r="1.1"/><circle cx="6.5" cy="10.5" r="1.1"/></svg>';
    menuBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      openProjectMenu(menuBtn, project);
    });
    actionGroup.appendChild(menuBtn);

    projHeader.appendChild(actionGroup);

    section.appendChild(projHeader);

    // Task list (already sorted above)
    if (!isCollapsed) {
      const taskList = el('div', { class: 'agent-project__tasks' });
      for (const task of projectTasks) {
        taskList.appendChild(buildTaskRow(task, activeTaskId, false));
      }
      section.appendChild(taskList);
    }


    // Inline terminals section
    if (expandedTerminals.has(project.id)) {
      const termSection = el('div', { class: 'agent-project__inline-section' });
      termSection.appendChild(el('div', { class: 'agent-project__inline-label' }, 'Terminals'));
      const termList = el('div', { class: 'agent-project__inline-terminals' });
      termList.appendChild(el('div', { class: 'agent-project__inline-empty' }, 'Loading...'));
      termSection.appendChild(termList);
      section.appendChild(termSection);

      api.listTerminals().then(terminals => {
        termList.innerHTML = '';
        if (!terminals || terminals.length === 0) {
          termList.appendChild(el('div', { class: 'agent-project__inline-empty' }, 'No agent terminals'));
          return;
        }
        for (const term of terminals) {
          const row = el('div', { class: 'agent-terminal-row' });
          row.appendChild(el('span', { class: 'agent-terminal-row__label' }, term.label || term.session_id || term.id || 'Terminal'));
          if (term.cwd) row.appendChild(el('span', { class: 'agent-terminal-row__cwd' }, term.cwd));
          const sessionId = term.session_id || term.id;
          row.addEventListener('click', () => {
            document.dispatchEvent(new CustomEvent('focus-terminal', { detail: { sessionId } }));
          });
          termList.appendChild(row);
        }
      }).catch(err => {
        console.error('Failed to list terminals:', err);
        termList.innerHTML = '';
        termList.appendChild(el('div', { class: 'agent-project__inline-empty' }, 'Failed to load terminals'));
      });
    }

    return section;
  }

  function buildTaskRow(task, activeTaskId, isHistory) {
    const taskEl = el('div', {
      class: `agent-task ${task.id === activeTaskId ? 'agent-task--active' : ''}`,
    });

    taskEl.appendChild(makeStatusDot(task.status));

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

  // ── Main render ───────────────────────────────────────────

  async function loadProjectTasks(projectId) {
    if (loadedProjectIds.has(projectId)) return;
    loadedProjectIds.add(projectId);
    try {
      const infos = await api.listTasks(projectId);
      if (!infos?.length) return;
      const tasks = { ...agentStore.getState('tasks') };
      let changed = false;
      const newIds = [];
      for (const info of infos) {
        if (!tasks[info.id]) {
          tasks[info.id] = { ...info, messages: [], isStreaming: false };
          newIds.push(info.id);
          changed = true;
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
  workspaceStore.subscribe('projects', () => renderContent());

  panel.appendChild(header);
  panel.appendChild(content);

  renderContent();
  initAgentEvents();

  return panel;
}
