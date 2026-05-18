import { el, icon, iconMulti } from '../../utils/dom.js';
import { gitStore, refreshAllGitStatuses } from '../../state/git.js';
import { workspaceStore, addProject, cloneAndAddProject } from '../../state/workspace.js';
import { openDiffView } from '../../state/editor.js';
import { createProjectScm } from './project-scm.js';
import { createConflictPanel } from './conflict-panel.js';
import { createCommitHistory } from './commit-history.js';
import * as api from '../../lib/tauri-api.js';

// Icon path data (Feather-style, viewBox 0 0 24 24)
const CHANGES_ICON = [
  // edit / pencil-on-paper
  'M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7',
  'M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z',
];
const COMMITS_ICON = [
  // git-commit (horizontal line with circle)
  'M1.05 12H7',
  'M17 12h5.95',
  'M12 12m-5 0a5 5 0 1 0 10 0 5 5 0 1 0-10 0',
];
const FOLDER_PLUS_ICON = [
  'M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z',
  'M12 11v6',
  'M9 14h6',
];
const CLONE_ICON = [
  'M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4',
  'M7 10l5 5 5-5',
  'M12 15V3',
];

async function showCloneDialog() {
  let defaultDir = '';
  try { defaultDir = await api.getDefaultProjectsDir(); } catch {}

  return new Promise((resolve) => {
    let resolved = false;

    function finish(result) {
      if (resolved) return;
      resolved = true;
      overlay.remove();
      document.removeEventListener('keydown', onKey);
      resolve(result);
    }

    function onKey(e) {
      if (e.key === 'Escape') { e.preventDefault(); finish(null); }
      if (e.key === 'Enter') { e.preventDefault(); submit(); }
    }

    async function submit() {
      const url = urlInput.value.trim();
      if (!url) { urlInput.focus(); return; }
      const target = dirInput.value.trim() || null;
      cloneBtn.disabled = true;
      cloneBtn.textContent = 'Cloning…';
      statusEl.textContent = '';
      try {
        await cloneAndAddProject(url, target);
        finish(true);
      } catch (e) {
        cloneBtn.disabled = false;
        cloneBtn.textContent = 'Clone';
        statusEl.textContent = String(e);
      }
    }

    const overlay = el('div', { class: 'confirm-dialog-overlay' });
    const dialog = el('div', { class: 'confirm-dialog' });

    dialog.appendChild(el('div', { class: 'confirm-dialog__title' }, 'Clone Repository'));

    const form = el('div', { style: 'display:flex;flex-direction:column;gap:8px;margin-bottom:12px' });

    const urlLabel = el('label', { style: 'font-size:11px;color:var(--text-muted)' }, 'Repository URL');
    const urlInput = el('input', {
      type: 'text',
      placeholder: 'https://github.com/user/repo.git',
      style: 'width:100%;box-sizing:border-box;padding:5px 8px;background:var(--input-bg,var(--bg-secondary));border:1px solid var(--border-color);border-radius:4px;color:var(--text-primary);font-size:12px',
    });

    const dirLabel = el('label', { style: 'font-size:11px;color:var(--text-muted)' }, `Clone into (default: ${defaultDir || '~/projects'})`);
    const dirInput = el('input', {
      type: 'text',
      placeholder: defaultDir || '~/projects',
      style: 'width:100%;box-sizing:border-box;padding:5px 8px;background:var(--input-bg,var(--bg-secondary));border:1px solid var(--border-color);border-radius:4px;color:var(--text-primary);font-size:12px',
    });

    const statusEl = el('div', { style: 'font-size:11px;color:var(--error-color,#f55);min-height:14px;word-break:break-all' });

    form.appendChild(urlLabel);
    form.appendChild(urlInput);
    form.appendChild(dirLabel);
    form.appendChild(dirInput);
    form.appendChild(statusEl);
    dialog.appendChild(form);

    const actions = el('div', { class: 'confirm-dialog__actions' });
    const cancelBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--cancel' }, 'Cancel');
    const cloneBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--save' }, 'Clone');

    cancelBtn.addEventListener('click', () => finish(null));
    cloneBtn.addEventListener('click', submit);

    actions.appendChild(cancelBtn);
    actions.appendChild(cloneBtn);
    dialog.appendChild(actions);

    overlay.appendChild(dialog);
    overlay.addEventListener('click', (e) => { if (e.target === overlay) finish(null); });
    document.body.appendChild(overlay);
    document.addEventListener('keydown', onKey);
    urlInput.focus();
  });
}

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

  // Add project (folder-plus) — same as in Explorer / Agent panel
  const addProjectBtn = el('button', { class: 'scm-header-action', title: 'Add Project Folder' });
  addProjectBtn.appendChild(iconMulti(FOLDER_PLUS_ICON, 14));
  addProjectBtn.addEventListener('click', () => addProject());

  // Clone repository button
  const cloneBtn = el('button', { class: 'scm-header-action', title: 'Clone Repository' });
  cloneBtn.appendChild(iconMulti(CLONE_ICON, 14));
  cloneBtn.addEventListener('click', () => showCloneDialog());

  headerActions.appendChild(refreshBtn);
  headerActions.appendChild(cloneBtn);
  headerActions.appendChild(addProjectBtn);
  header.appendChild(headerActions);

  const content = el('div', { class: 'source-control-content' });

  // Track expanded state and active tab per project
  const expandedState = {};
  const activeTabState = {}; // 'changes' or 'commits'

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
    const allUnpushed = gitStore.getState('projectUnpushedCommits');
    const allSyncStatus = gitStore.getState('projectSyncStatus');

    if (projects.length === 0) {
      const placeholder = el('div', { class: 'panel-placeholder', style: 'display:flex;flex-direction:column;align-items:center;gap:8px;padding:16px' });
      placeholder.appendChild(el('span', {}, 'No projects open'));
      const cloneLink = el('button', {
        style: 'background:none;border:none;color:var(--link-color,var(--accent));cursor:pointer;font-size:12px;text-decoration:underline;padding:0',
      }, 'Clone a Repository…');
      cloneLink.addEventListener('click', () => showCloneDialog());
      placeholder.appendChild(cloneLink);
      content.appendChild(placeholder);
      return;
    }

    for (const project of projects) {
      const status = statuses[project.id];
      const conflicts = allConflicts[project.id];
      const commits = allCommits[project.id];
      const unpushedCommits = allUnpushed[project.id];
      const syncStatus = allSyncStatus[project.id];
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
        class: `scm-tab-slider__tab scm-tab-slider__tab--icon${activeTab === 'changes' ? ' scm-tab-slider__tab--active' : ''}`,
        title: fileCount > 0 ? `Changes (${fileCount})` : 'Changes',
      });
      changesTab.appendChild(iconMulti(CHANGES_ICON, 13));
      if (fileCount > 0) {
        changesTab.appendChild(el('span', { class: 'scm-tab-slider__badge' }, String(fileCount)));
      }
      changesTab.addEventListener('click', (e) => { e.stopPropagation(); setActiveTab(project.id, 'changes'); });

      const commitsTab = el('button', {
        class: `scm-tab-slider__tab scm-tab-slider__tab--icon${activeTab === 'commits' ? ' scm-tab-slider__tab--active' : ''}`,
        title: commitCount > 0 ? `Commits (${commitCount})` : 'Commits',
      });
      commitsTab.appendChild(iconMulti(COMMITS_ICON, 13));
      if (commitCount > 0) {
        commitsTab.appendChild(el('span', { class: 'scm-tab-slider__badge' }, String(commitCount)));
      }
      commitsTab.addEventListener('click', (e) => { e.stopPropagation(); setActiveTab(project.id, 'commits'); });

      tabSlider.appendChild(changesTab);
      tabSlider.appendChild(commitsTab);

      headerRow.appendChild(headerLeft);
      headerRow.appendChild(tabSlider);
      section.appendChild(headerRow);

      // Expanded content
      if (expanded && status) {
        if (activeTab === 'changes') {
          if (conflicts && conflicts.length > 0) {
            section.appendChild(createConflictPanel(project, conflicts));
          }
          section.appendChild(createProjectScm(project, status, unpushedCommits, syncStatus, onFileClick));
        } else {
          section.appendChild(createCommitHistory(project.id, commits, onCommitFileClick));
        }
      } else if (expanded && !status) {
        // Not a git repo
        section.appendChild(createProjectScm(project, status, unpushedCommits, onFileClick));
      }

      content.appendChild(section);
    }
  }

  gitStore.subscribe('projectStatuses', render);
  gitStore.subscribe('projectConflicts', render);
  gitStore.subscribe('projectCommits', render);
  gitStore.subscribe('projectUnpushedCommits', render);
  gitStore.subscribe('projectSyncStatus', render);
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
