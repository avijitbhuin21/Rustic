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

function makeStatusIcon(status) {
  const statusIcon = el('span', { class: 'agent-task__status' });
  if (status === 'Running') {
    statusIcon.innerHTML = '<span class="agent-task__spinner"></span>';
  } else if (status === 'Completed') {
    statusIcon.appendChild(icon('M5 12l5 5L20 7', 12));
    statusIcon.style.color = 'var(--bright-green)';
  } else if (status === 'Failed') {
    statusIcon.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
    statusIcon.style.color = 'var(--bright-red)';
  } else if (status === 'Cancelled' || status === 'Stopped') {
    statusIcon.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
    statusIcon.style.color = 'var(--fg4)';
  } else if (status === 'TurnLimitReached') {
    statusIcon.appendChild(icon('M12 9v4m0 4h.01M12 2a10 10 0 1 0 0 20A10 10 0 0 0 12 2z', 12));
    statusIcon.style.color = 'var(--bright-yellow)';
  } else {
    statusIcon.appendChild(icon('M12 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0', 12));
  }
  return statusIcon;
}

export function createAgentPanel() {
  const panel = el('div', { class: 'agent-panel' });

  // ── Local state ───────────────────────────────────────────
  const collapsedProjects = new Set();
  const expandedHistory = new Set();
  const expandedTerminals = new Set();

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
      expandedHistory.has(project.id),
      () => {
        if (expandedHistory.has(project.id)) expandedHistory.delete(project.id);
        else expandedHistory.add(project.id);
        renderContent();
      }
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

  // ── Project sections ──────────────────────────────────────

  function buildProjectSection(project, tasks, activeTaskId) {
    const projectTasks = Object.values(tasks).filter(
      t => (t.project_id === project.id || t.projectId === project.id)
        && !TERMINAL_STATUSES.has(t.status)
    );

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

    // Active task list
    if (!isCollapsed) {
      const taskList = el('div', { class: 'agent-project__tasks' });
      const sorted = [...projectTasks].sort((a, b) => (a.status === 'Running' ? 0 : 1) - (b.status === 'Running' ? 0 : 1));
      for (const task of sorted) {
        taskList.appendChild(buildTaskRow(task, activeTaskId, false));
      }
      section.appendChild(taskList);
    }

    // Inline history section
    if (expandedHistory.has(project.id)) {
      const histSection = el('div', { class: 'agent-project__inline-section' });
      histSection.appendChild(el('div', { class: 'agent-project__inline-label' }, 'History'));
      const histTasks = Object.values(tasks).filter(
        t => (t.project_id === project.id || t.projectId === project.id) && TERMINAL_STATUSES.has(t.status)
      );
      if (histTasks.length === 0) {
        histSection.appendChild(el('div', { class: 'agent-project__inline-empty' }, 'No history yet'));
      } else {
        for (const task of histTasks) {
          histSection.appendChild(buildTaskRow(task, activeTaskId, true));
        }
      }
      section.appendChild(histSection);
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

    taskEl.appendChild(makeStatusIcon(task.status));

    const titleEl = el('span', { class: 'agent-task__title' }, task.title);
    taskEl.appendChild(titleEl);

    const costStr = formatCost(task.cost);
    if (costStr) {
      taskEl.appendChild(el('span', { class: 'agent-task__cost' }, costStr));
    }

    if (!isHistory) {
      const statusLabel = el('span', { class: 'agent-task__status-label' }, task.status || 'Idle');
      taskEl.appendChild(statusLabel);

      if (task.status === 'Running') {
        const stopBtn = el('button', { class: 'agent-task__stop', title: 'Stop Task' });
        stopBtn.appendChild(icon('M6 6h12v12H6z', 10));
        stopBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          api.abortTask(task.id).catch(err => console.error('Failed to abort task:', err));
        });
        taskEl.appendChild(stopBtn);
      }
    } else {
      if (task.updated_at || task.created_at) {
        const dateStr = formatDate(task.updated_at || task.created_at);
        if (dateStr) {
          taskEl.appendChild(el('span', { class: 'agent-task__date' }, dateStr));
        }
      }
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
