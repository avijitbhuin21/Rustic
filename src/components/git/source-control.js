import { el, icon } from '../../utils/dom.js';
import { gitStore, refreshAllGitStatuses, refreshGitStatus } from '../../state/git.js';
import { workspaceStore } from '../../state/workspace.js';
import { openDiffView } from '../../state/editor.js';
import { createProjectScm } from './project-scm.js';
import { createConflictPanel } from './conflict-panel.js';
import { createCommitHistory } from './commit-history.js';

export function createSourceControl() {
  const panel = el('div', { class: 'source-control-panel' });

  // Header row: "SOURCE CONTROL" + refresh icon only
  const header = el('div', { class: 'sidebar-header' }, [
    el('span', {}, 'Source Control'),
  ]);

  const headerActions = el('div', { class: 'scm-header-actions' });

  // Global refresh
  const refreshBtn = el('button', { class: 'scm-header-action', title: 'Refresh All' });
  refreshBtn.appendChild(icon('M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15', 14));
  refreshBtn.addEventListener('click', async () => {
    refreshBtn.classList.add('spinning');
    const projects = workspaceStore.getState('projects');
    const minSpin = new Promise(r => setTimeout(r, 600));
    await Promise.all([refreshAllGitStatuses(projects), minSpin]);
    refreshBtn.classList.remove('spinning');
  });

  headerActions.appendChild(refreshBtn);
  header.appendChild(headerActions);

  const content = el('div', { class: 'source-control-content' });

  // Track expanded state and active tab per project
  const expandedState = {};
  const activeTabState = {}; // 'changes' or 'commits'
  const spinningState = {};  // projectId -> true while refreshing

  function toggleProject(projectId) {
    expandedState[projectId] = !expandedState[projectId];
    render();
  }

  function isExpanded(projectId) {
    if (expandedState[projectId] === undefined) expandedState[projectId] = true;
    return expandedState[projectId];
  }

  function getActiveTab(projectId) {
    return activeTabState[projectId] || 'changes';
  }

  function setActiveTab(projectId, tab) {
    activeTabState[projectId] = tab;
    render();
  }

  function onFileClick(projectId, filePath, isStaged) {
    openDiffView({ projectId, filePath, isStaged });
  }

  function onCommitFileClick(projectId, filePath, oid) {
    openDiffView({ projectId, filePath, oid });
  }

  function render() {
    content.innerHTML = '';
    const projects = workspaceStore.getState('projects');
    const statuses = gitStore.getState('projectStatuses');
    const allConflicts = gitStore.getState('projectConflicts');
    const allCommits = gitStore.getState('projectCommits');

    if (projects.length === 0) {
      content.appendChild(el('div', { class: 'panel-placeholder' }, 'No projects open'));
      return;
    }

    for (const project of projects) {
      const status = statuses[project.id];
      const conflicts = allConflicts[project.id];
      const commits = allCommits[project.id];
      const expanded = isExpanded(project.id);
      const activeTab = getActiveTab(project.id);

      // Project section wrapper
      const section = el('div', { class: 'scm-project-section' });

      // Project header
      const headerRow = el('div', { class: 'scm-project-section__header' });

      const caretIcon = icon(expanded ? 'M6 9l6 6 6-6' : 'M9 18l6-6-6-6', 12);
      const caret = el('span', { class: 'scm-project-section__caret' }, caretIcon);

      const nameEl = el('span', { class: 'scm-project-section__name' }, project.name);

      const headerLeft = el('div', { class: 'scm-project-section__header-left' });
      headerLeft.appendChild(caret);
      headerLeft.appendChild(nameEl);
      headerLeft.addEventListener('click', () => toggleProject(project.id));

      // Tab slider: Changes | Commits (replaces branch name)
      const fileCount = status ? status.files.length : 0;
      const commitCount = commits ? commits.length : 0;

      const tabSlider = el('div', { class: 'scm-tab-slider' });

      const changesTab = el('button', {
        class: `scm-tab-slider__tab${activeTab === 'changes' ? ' scm-tab-slider__tab--active' : ''}`,
      });
      changesTab.appendChild(el('span', {}, 'Changes'));
      if (fileCount > 0) {
        changesTab.appendChild(el('span', { class: 'scm-tab-slider__badge' }, String(fileCount)));
      }
      changesTab.addEventListener('click', (e) => { e.stopPropagation(); setActiveTab(project.id, 'changes'); });

      const commitsTab = el('button', {
        class: `scm-tab-slider__tab${activeTab === 'commits' ? ' scm-tab-slider__tab--active' : ''}`,
      });
      commitsTab.appendChild(el('span', {}, 'Commits'));
      if (commitCount > 0) {
        commitsTab.appendChild(el('span', { class: 'scm-tab-slider__badge' }, String(commitCount)));
      }
      commitsTab.addEventListener('click', (e) => { e.stopPropagation(); setActiveTab(project.id, 'commits'); });

      tabSlider.appendChild(changesTab);
      tabSlider.appendChild(commitsTab);

      // Per-project action buttons
      const actions = el('div', { class: 'scm-project-section__actions' });

      const projRefreshBtn = createProjectAction('Refresh', 'M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15', async () => {
        if (spinningState[project.id]) return;
        spinningState[project.id] = true;
        render();
        const minSpin = new Promise(r => setTimeout(r, 600));
        await Promise.all([refreshGitStatus(project.id), minSpin]);
        spinningState[project.id] = false;
        render();
      });
      if (spinningState[project.id]) projRefreshBtn.classList.add('spinning');
      actions.appendChild(projRefreshBtn);

      headerRow.appendChild(headerLeft);
      headerRow.appendChild(tabSlider);
      headerRow.appendChild(actions);
      section.appendChild(headerRow);

      // Expanded content
      if (expanded && status) {
        if (activeTab === 'changes') {
          if (conflicts && conflicts.length > 0) {
            section.appendChild(createConflictPanel(project, conflicts));
          }
          section.appendChild(createProjectScm(project, status, onFileClick));
        } else {
          section.appendChild(createCommitHistory(project.id, commits, onCommitFileClick));
        }
      } else if (expanded && !status) {
        // Not a git repo
        section.appendChild(createProjectScm(project, status, onFileClick));
      }

      content.appendChild(section);
    }
  }

  gitStore.subscribe('projectStatuses', render);
  gitStore.subscribe('projectConflicts', render);
  gitStore.subscribe('projectCommits', render);
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

function createProjectAction(title, iconPath, onClick) {
  const btn = el('button', {
    class: 'scm-project-section__action-btn',
    title,
    onClick,
  });
  btn.appendChild(icon(iconPath, 14));
  return btn;
}
