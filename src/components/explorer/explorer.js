import { el, iconMulti } from '../../utils/dom.js';
import { workspaceStore, addProject } from '../../state/workspace.js';
import { createProjectSection } from './project-section.js';
import { debug } from '../../lib/log.js';

export function createExplorer() {
  const container = el('div', { class: 'explorer' });

  const header = el('div', { class: 'sidebar-header' }, [
    el('span', {}, 'Explorer'),
  ]);

  const addBtn = el('button', {
    class: 'sidebar-header__action',
    title: 'Add Project Folder',
  });
  addBtn.appendChild(iconMulti([
    'M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z',
    'M12 11v6',
    'M9 14h6',
  ], 14));
  addBtn.addEventListener('click', () => addProject());
  header.appendChild(addBtn);

  const content = el('div', { class: 'explorer__content scrollable' });

  container.appendChild(header);
  container.appendChild(content);

  function render() {
    debug('FileTree', 'explorer FULL render triggered');
    const projects = workspaceStore.getState('projects');
    content.innerHTML = '';

    if (!projects || projects.length === 0) {
      content.appendChild(
        el('div', { class: 'explorer__empty' }, [
          el('p', {}, 'No projects open'),
          el('button', {
            class: 'explorer__add-btn',
            onClick: () => addProject(),
          }, 'Add Project Folder'),
        ])
      );
      return;
    }

    for (const project of projects) {
      content.appendChild(createProjectSection(project));
    }
  }

  workspaceStore.subscribe('projects', render);
  render();

  return container;
}
