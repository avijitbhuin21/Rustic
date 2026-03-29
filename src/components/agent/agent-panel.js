import { el, icon } from '../../utils/dom.js';
import { agentStore, createTask, setActiveTask, deleteTaskAction, initAgentEvents } from '../../state/agent.js';
import { workspaceStore } from '../../state/workspace.js';
import { openSettings, setCategory } from '../../state/settings.js';

export function createAgentPanel() {
  const panel = el('div', { class: 'agent-panel' });

  // Header
  const header = el('div', { class: 'sidebar-header' });
  header.appendChild(el('span', {}, 'Agent'));

  const headerActions = el('div', { class: 'sidebar-header__actions' });

  function makeHeaderBtn(title, svgPath, section) {
    const btn = el('button', { class: 'sidebar-header__action', title });
    btn.appendChild(icon(svgPath, 13));
    btn.addEventListener('click', () => {
      setCategory('agent');
      openSettings();
    });
    return btn;
  }

  // Configure Providers
  headerActions.appendChild(makeHeaderBtn(
    'Configure Providers',
    'M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4',
    'providers'
  ));
  // MCP Servers
  headerActions.appendChild(makeHeaderBtn(
    'MCP Servers',
    'M5 12H3m16 0h-2M12 5V3m0 16v-2m-4.95-1.05-1.414 1.414M18.364 5.636l-1.414 1.414M18.364 18.364l-1.414-1.414M6.05 6.05 4.636 4.636M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z',
    'mcp'
  ));
  // Skills
  headerActions.appendChild(makeHeaderBtn(
    'Skills',
    'M13 10V3L4 14h7v7l9-11h-7z',
    'skills'
  ));
  // Workflows
  headerActions.appendChild(makeHeaderBtn(
    'Workflows',
    'M6 3v12M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM15 6h-3a6 6 0 0 0-6 6v3',
    'workflows'
  ));

  header.appendChild(headerActions);

  const content = el('div', { class: 'agent-panel__content' });

  function render() {
    content.innerHTML = '';
    const projects = workspaceStore.getState('projects');
    const tasks = agentStore.getState('tasks');
    const activeTaskId = agentStore.getState('activeTaskId');

    if (projects.length === 0) {
      content.appendChild(el('div', { class: 'panel-placeholder' }, 'No projects open'));
      return;
    }

    for (const project of projects) {
      const section = el('div', { class: 'agent-project' });

      // Project header
      const projHeader = el('div', { class: 'agent-project__header' });
      projHeader.appendChild(el('span', { class: 'agent-project__name' }, project.name));

      const newBtn = el('button', { class: 'agent-project__new', title: 'New Task' });
      newBtn.appendChild(icon('M12 5v14M5 12h14', 12));
      newBtn.addEventListener('click', () => {
        const title = prompt('Task title:', 'New Task');
        if (title) createTask(project.id, title);
      });
      projHeader.appendChild(newBtn);

      section.appendChild(projHeader);

      // Task list for this project
      const projectTasks = Object.values(tasks).filter(t => t.project_id === project.id || t.projectId === project.id);
      for (const task of projectTasks) {
        const taskEl = el('div', {
          class: `agent-task ${task.id === activeTaskId ? 'agent-task--active' : ''}`,
        });

        // Status icon
        const statusIcon = el('span', { class: 'agent-task__status' });
        if (task.status === 'Running') {
          statusIcon.innerHTML = '<span class="agent-task__spinner"></span>';
        } else if (task.status === 'Completed') {
          statusIcon.appendChild(icon('M5 12l5 5L20 7', 12));
          statusIcon.style.color = 'var(--bright-green)';
        } else if (task.status === 'Failed') {
          statusIcon.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
          statusIcon.style.color = 'var(--bright-red)';
        } else {
          statusIcon.appendChild(icon('M12 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0', 12));
        }

        const titleEl = el('span', { class: 'agent-task__title' }, task.title);

        const deleteBtn = el('button', { class: 'agent-task__delete', title: 'Delete Task' });
        deleteBtn.appendChild(icon('M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', 10));
        deleteBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          deleteTaskAction(task.id);
        });

        taskEl.appendChild(statusIcon);
        taskEl.appendChild(titleEl);
        taskEl.appendChild(deleteBtn);

        taskEl.addEventListener('click', () => setActiveTask(task.id));
        section.appendChild(taskEl);
      }

      content.appendChild(section);
    }
  }

  agentStore.subscribe('tasks', render);
  agentStore.subscribe('activeTaskId', render);
  workspaceStore.subscribe('projects', render);

  panel.appendChild(header);
  panel.appendChild(content);

  render();
  initAgentEvents();

  return panel;
}
