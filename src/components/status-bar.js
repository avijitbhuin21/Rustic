import { el, icon } from '../utils/dom.js';
import { editorStore } from '../state/editor.js';
import { gitStore, pushChanges, pullChanges, fetchChanges, publishBranch } from '../state/git.js';
import { workspaceStore } from '../state/workspace.js';
import { createBranchSwitcher } from './branch-switcher.js';
import { showContextMenu } from './dropdown-menu.js';

export function createStatusBar() {
  const bar = el('div', { class: 'status-bar' });

  /// Determine which project the status-bar segments should reflect.
  /// Priority:
  ///   1. Project containing the active editor buffer (if any).
  ///   2. The locally-pinned project (set via the project pill).
  ///   3. projects[0] (legacy fallback).
  let pinnedProjectId = null;
  function getFocusProject() {
    const projects = workspaceStore.getState('projects') || [];
    if (projects.length === 0) return null;

    const bufferId = editorStore.getState('activeBufferId');
    const buffers = editorStore.getState('openBuffers') || {};
    const buffer = buffers[bufferId];
    if (buffer?.filePath || buffer?.file_path) {
      const path = (buffer.filePath || buffer.file_path).replace(/\\/g, '/');
      // Find the longest matching project root so nested workspaces resolve
      // to the deeper project rather than the parent.
      let best = null;
      for (const p of projects) {
        if (!p.root_path) continue;
        const root = p.root_path.replace(/\\/g, '/');
        if (path === root || path.startsWith(root + '/')) {
          if (!best || root.length > best.root_path.replace(/\\/g, '/').length) {
            best = p;
          }
        }
      }
      if (best) return best;
    }

    if (pinnedProjectId) {
      const pinned = projects.find((p) => p.id === pinnedProjectId);
      if (pinned) return pinned;
    }
    return projects[0];
  }

  // Project picker (only renders when 2+ projects). Click opens a menu of
  // all projects so the user can pin which one drives the status bar.
  const projectPicker = el('span', {
    class: 'status-bar__segment status-bar__clickable status-bar__project-pill',
    title: 'Switch the project this status-bar segment reflects',
  });
  projectPicker.appendChild(icon('M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z', 11));
  const projectName = el('span', { class: 'status-bar__project-name' }, '');
  projectPicker.appendChild(projectName);
  projectPicker.appendChild(icon('M19 9l-7 7-7-7', 9));
  projectPicker.addEventListener('click', (e) => {
    e.stopPropagation();
    const projects = workspaceStore.getState('projects') || [];
    const focus = getFocusProject();
    const items = projects
      .filter((p) => p.id !== '__global__')
      .map((p) => ({
        label: p.name || p.root_path,
        description: p.id === focus?.id ? '✓' : '',
        action: () => {
          pinnedProjectId = p.id;
          updateAll();
        },
      }));
    if (items.length === 0) return;
    const rect = projectPicker.getBoundingClientRect();
    showContextMenu(items, rect.left, rect.top);
  });

  // Branch + branch-switch click target.
  const branchEl = el('span', { class: 'status-bar__segment status-bar__clickable status-bar__branch' });
  const branchIcon = icon('M6 3v12M18 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9', 10);
  const branchText = el('span', {}, '');
  // Sync indicator is a sibling, NOT a child, of the branch text so it can
  // be its own click target (Push/Pull/Fetch menu) without triggering the
  // branch switcher.
  const syncEl = el('span', { class: 'status-bar__sync status-bar__clickable' }, '');
  branchEl.appendChild(branchIcon);
  branchEl.appendChild(branchText);

  const openBranchSwitcher = (e) => {
    e.stopPropagation();
    const focus = getFocusProject();
    if (focus) createBranchSwitcher(focus.id, branchEl);
  };
  branchText.addEventListener('click', openBranchSwitcher);
  branchIcon.addEventListener('click', openBranchSwitcher);

  syncEl.addEventListener('click', (e) => {
    e.stopPropagation();
    const project = getFocusProject();
    if (!project) return;
    const sync = (gitStore.getState('projectSyncStatus') || {})[project.id] || {};
    const ahead = sync.ahead || 0;
    const behind = sync.behind || 0;
    const noUpstream = sync.has_upstream === false;

    const items = [];
    if (noUpstream) {
      items.push({ label: 'Branch not yet published', disabled: true });
      items.push({ separator: true });
      items.push({
        label: 'Publish Branch',
        action: () => publishBranch(project.id),
      });
    } else {
      if (ahead > 0) {
        items.push({
          label: `Push ${ahead} commit${ahead === 1 ? '' : 's'}`,
          action: () => pushChanges(project.id),
        });
      }
      if (behind > 0) {
        items.push({
          label: `Pull ${behind} commit${behind === 1 ? '' : 's'}`,
          action: () => pullChanges(project.id),
        });
      }
      if (ahead > 0 && behind > 0) {
        items.push({ separator: true });
      }
      items.push({
        label: 'Fetch',
        action: () => fetchChanges(project.id),
      });
      if (ahead === 0 && behind === 0) {
        items.unshift({ label: 'Already up to date', disabled: true });
        items.push({ separator: true });
      }
    }
    const rect = syncEl.getBoundingClientRect();
    showContextMenu(items, rect.left, rect.top);
  });

  const branchRow = el('span', { class: 'status-bar__branch-row' }, [projectPicker, branchEl, syncEl]);

  const errorsEl = el('span', { class: 'status-bar__segment' }, '');
  const savedEl = el('span', { class: 'status-bar__segment status-bar__saved' }, '');
  const left = el('div', { class: 'status-bar__left' }, [branchRow, errorsEl, savedEl]);

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

  function updateProjectPicker() {
    const projects = (workspaceStore.getState('projects') || [])
      .filter((p) => p.id !== '__global__');
    const focus = getFocusProject();
    if (projects.length < 2 || !focus) {
      projectPicker.style.display = 'none';
      return;
    }
    projectPicker.style.display = '';
    projectName.textContent = focus.name || focus.root_path;
    projectPicker.title = `${focus.name || focus.root_path} — click to switch`;
  }

  function updateBranch() {
    const projects = workspaceStore.getState('projects');
    const statuses = gitStore.getState('projectStatuses');
    const syncStatuses = gitStore.getState('projectSyncStatus');

    if (projects.length === 0) {
      branchText.textContent = '';
      syncEl.textContent = '';
      branchEl.style.display = 'none';
      syncEl.style.display = 'none';
      return;
    }

    const focus = getFocusProject();
    if (!focus) {
      branchEl.style.display = 'none';
      syncEl.style.display = 'none';
      return;
    }
    const status = statuses[focus.id];
    if (status && status.branch) {
      branchText.textContent = status.branch;
      branchEl.style.display = '';

      const sync = syncStatuses[focus.id];
      if (sync?.has_upstream === false) {
        syncEl.textContent = '↑ Publish';
        syncEl.title = 'Branch has no upstream — click to publish';
        syncEl.style.display = '';
      } else if (sync && (sync.ahead > 0 || sync.behind > 0)) {
        let syncText = '';
        if (sync.ahead > 0) syncText += `↑${sync.ahead}`;
        if (sync.behind > 0) syncText += ` ↓${sync.behind}`;
        syncEl.textContent = syncText;
        syncEl.title = sync.ahead > 0 && sync.behind > 0
          ? `${sync.ahead} ahead, ${sync.behind} behind — click to push/pull`
          : sync.ahead > 0
            ? `${sync.ahead} unpushed commit${sync.ahead === 1 ? '' : 's'} — click to push`
            : `${sync.behind} commit${sync.behind === 1 ? '' : 's'} behind — click to pull`;
        syncEl.style.display = '';
      } else {
        syncEl.textContent = '↕';
        syncEl.title = 'Up to date — click to fetch';
        syncEl.style.display = '';
      }
    } else {
      branchEl.style.display = 'none';
      syncEl.style.display = 'none';
    }
  }

  function updateAll() {
    updateProjectPicker();
    updateBranch();
  }

  // Transient "Saved" pill triggered by saveBuffer dispatching a CustomEvent.
  let savedTimer = null;
  function flashSaved(detail) {
    savedEl.innerHTML = '';
    savedEl.appendChild(icon('M5 13l4 4L19 7', 11));
    const label = detail?.fileName ? `Saved ${detail.fileName}` : 'Saved';
    savedEl.appendChild(el('span', {}, label));
    savedEl.classList.add('status-bar__saved--visible');
    if (savedTimer) clearTimeout(savedTimer);
    savedTimer = setTimeout(() => {
      savedEl.classList.remove('status-bar__saved--visible');
    }, 1600);
  }
  window.addEventListener('rustic:buffer-saved', (e) => {
    flashSaved(e.detail || {});
  });

  editorStore.subscribe('activeBufferId', () => { updateCursor(); updateAll(); });
  editorStore.subscribe('cursorLine', updateCursor);
  editorStore.subscribe('cursorCol', updateCursor);
  editorStore.subscribe('openBuffers', updateAll);
  gitStore.subscribe('projectStatuses', updateBranch);
  gitStore.subscribe('projectSyncStatus', updateBranch);
  workspaceStore.subscribe('projects', updateAll);

  updateCursor();
  updateAll();

  return bar;
}
