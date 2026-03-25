import { el, icon } from '../utils/dom.js';
import { editorStore } from '../state/editor.js';
import { gitStore } from '../state/git.js';
import { workspaceStore } from '../state/workspace.js';
import { createBranchSwitcher } from './branch-switcher.js';

export function createStatusBar() {
  const bar = el('div', { class: 'status-bar' });

  // Left: branch + sync + errors/warnings
  const branchEl = el('span', { class: 'status-bar__segment status-bar__clickable status-bar__branch' });
  const branchIcon = icon('M6 3v12M18 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9', 10);
  const branchText = el('span', {}, '');
  const syncEl = el('span', { class: 'status-bar__sync' }, '');
  branchEl.appendChild(branchIcon);
  branchEl.appendChild(branchText);
  branchEl.appendChild(syncEl);

  branchEl.addEventListener('click', (e) => {
    e.stopPropagation();
    const projects = workspaceStore.getState('projects');
    if (projects.length > 0) {
      createBranchSwitcher(projects[0].id, branchEl);
    }
  });

  const errorsEl = el('span', { class: 'status-bar__segment' }, '');
  const left = el('div', { class: 'status-bar__left' }, [branchEl, errorsEl]);

  // Right: cursor position, language, encoding, line ending, indentation
  const cursorEl = el('span', { class: 'status-bar__segment status-bar__clickable' }, '');
  const languageEl = el('span', { class: 'status-bar__segment status-bar__clickable' }, '');
  const encodingEl = el('span', { class: 'status-bar__segment' }, 'UTF-8');
  const eolEl = el('span', { class: 'status-bar__segment' }, 'LF');
  const indentEl = el('span', { class: 'status-bar__segment' }, 'Spaces: 4');
  const right = el('div', { class: 'status-bar__right' }, [cursorEl, languageEl, encodingEl, eolEl, indentEl]);

  bar.appendChild(left);
  bar.appendChild(right);

  function updateCursor() {
    const bufferId = editorStore.getState('activeBufferId');
    const line = editorStore.getState('cursorLine') || 0;
    const col = editorStore.getState('cursorCol') || 0;

    cursorEl.textContent = bufferId ? `Ln ${line + 1}, Col ${col + 1}` : '';

    // Language from active buffer
    const buffers = editorStore.getState('openBuffers') || {};
    const buffer = buffers[bufferId];
    languageEl.textContent = buffer?.language || '';
  }

  function updateBranch() {
    const projects = workspaceStore.getState('projects');
    const statuses = gitStore.getState('projectStatuses');
    const syncStatuses = gitStore.getState('projectSyncStatus');

    if (projects.length === 0) {
      branchText.textContent = '';
      syncEl.textContent = '';
      branchEl.style.display = 'none';
      return;
    }

    // Show first project's branch
    const firstProject = projects[0];
    const status = statuses[firstProject.id];
    if (status && status.branch) {
      branchText.textContent = status.branch;
      branchEl.style.display = '';

      const sync = syncStatuses[firstProject.id];
      if (sync && (sync.ahead > 0 || sync.behind > 0)) {
        let syncText = '';
        if (sync.ahead > 0) syncText += `\u2191${sync.ahead}`;
        if (sync.behind > 0) syncText += ` \u2193${sync.behind}`;
        syncEl.textContent = syncText;
      } else {
        syncEl.textContent = '';
      }
    } else {
      branchEl.style.display = 'none';
    }
  }

  editorStore.subscribe('activeBufferId', updateCursor);
  editorStore.subscribe('cursorLine', updateCursor);
  editorStore.subscribe('cursorCol', updateCursor);
  gitStore.subscribe('projectStatuses', updateBranch);
  gitStore.subscribe('projectSyncStatus', updateBranch);
  workspaceStore.subscribe('projects', updateBranch);

  updateCursor();
  updateBranch();

  return bar;
}
