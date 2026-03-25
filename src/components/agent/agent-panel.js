import { el, icon } from '../../utils/dom.js';
import { agentStore, createTask, setActiveTask, deleteTaskAction, initAgentEvents } from '../../state/agent.js';
import { workspaceStore } from '../../state/workspace.js';
import { createMcpConfig } from './mcp-config.js';

export function createAgentPanel() {
  const panel = el('div', { class: 'agent-panel' });

  // Header
  const header = el('div', { class: 'sidebar-header' }, [
    el('span', {}, 'Agent'),
  ]);

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

  // MCP config section
  const mcpSection = createMcpConfig();

  panel.appendChild(header);
  panel.appendChild(content);
  panel.appendChild(mcpSection);

  render();
  initAgentEvents();

  return panel;
}
