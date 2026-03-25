import { el, icon } from '../../utils/dom.js';
import { gitStore, refreshAllGitStatuses, pushChanges, pullChanges, fetchChanges } from '../../state/git.js';
import { workspaceStore } from '../../state/workspace.js';
import { createProjectScm } from './project-scm.js';
import { createConflictPanel } from './conflict-panel.js';
import { createDropdownMenu } from '../dropdown-menu.js';

export function createSourceControl() {
  const panel = el('div', { class: 'source-control-panel' });

  // Header row: "SOURCE CONTROL" + action icons
  const header = el('div', { class: 'sidebar-header' }, [
    el('span', {}, 'Source Control'),
  ]);

  const headerActions = el('div', { class: 'scm-header-actions' });

  // Refresh
  const refreshBtn = el('button', { class: 'scm-header-action', title: 'Refresh' });
  refreshBtn.appendChild(icon('M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15', 14));
  refreshBtn.addEventListener('click', () => {
    const projects = workspaceStore.getState('projects');
    refreshAllGitStatuses(projects);
  });

  // More actions (...)
  const moreBtn = el('button', { class: 'scm-header-action', title: 'More Actions' });
  moreBtn.appendChild(icon('M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM19 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM5 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2z', 14));

  const moreDropdown = createDropdownMenu([
    { label: 'Pull', action: () => { const p = getFirstProject(); if (p) pullChanges(p.id).catch(() => {}); }},
    { label: 'Push', action: () => { const p = getFirstProject(); if (p) pushChanges(p.id).catch(() => {}); }},
    { label: 'Fetch', action: () => { const p = getFirstProject(); if (p) fetchChanges(p.id).catch(() => {}); }},
    { separator: true },
    { label: 'Refresh', action: () => { const projects = workspaceStore.getState('projects'); refreshAllGitStatuses(projects); }},
  ]);
  document.body.appendChild(moreDropdown.element);
  moreBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    const rect = moreBtn.getBoundingClientRect();
    moreDropdown.show(rect.left, rect.bottom + 2);
  });

  headerActions.appendChild(refreshBtn);
  headerActions.appendChild(moreBtn);
  header.appendChild(headerActions);

  const content = el('div', { class: 'source-control-content' });

  function getFirstProject() {
    const projects = workspaceStore.getState('projects');
    return projects.length > 0 ? projects[0] : null;
  }

  function render() {
    content.innerHTML = '';
    const projects = workspaceStore.getState('projects');
    const statuses = gitStore.getState('projectStatuses');
    const allConflicts = gitStore.getState('projectConflicts');

    if (projects.length === 0) {
      content.appendChild(el('div', { class: 'panel-placeholder' }, 'No projects open'));
      return;
    }

    for (const project of projects) {
      const status = statuses[project.id];
      const conflicts = allConflicts[project.id];

      if (conflicts && conflicts.length > 0) {
        content.appendChild(createConflictPanel(project, conflicts));
      }

      content.appendChild(createProjectScm(project, status));
    }
  }

  gitStore.subscribe('projectStatuses', render);
  gitStore.subscribe('projectConflicts', render);
  workspaceStore.subscribe('projects', (projects) => {
    render();
    refreshAllGitStatuses(projects);
  });

  panel.appendChild(header);
  panel.appendChild(content);

  const projects = workspaceStore.getState('projects');
  if (projects.length > 0) {
    render();
    refreshAllGitStatuses(projects);
  }

  return panel;
}
