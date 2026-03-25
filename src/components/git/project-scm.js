import { el, icon } from '../../utils/dom.js';
import {
  stageFiles, unstageFiles, commitChanges, discardChanges,
  commitAndPush, pushChanges, pullChanges, fetchChanges,
  initRepo, gitStore,
} from '../../state/git.js';
import { createDropdownMenu } from '../dropdown-menu.js';

const STATUS_ICONS = {
  New: { letter: 'A', color: 'var(--bright-green)' },
  Modified: { letter: 'M', color: 'var(--bright-yellow)' },
  Deleted: { letter: 'D', color: 'var(--bright-red)' },
  Renamed: { letter: 'R', color: 'var(--bright-blue)' },
  Untracked: { letter: 'U', color: 'var(--fg4)' },
  Conflicted: { letter: 'C', color: 'var(--bright-red)' },
};

// File extension icon colors (matching explorer)
const EXT_COLORS = {
  js: 'var(--bright-yellow)', ts: 'var(--bright-blue)',
  jsx: 'var(--bright-yellow)', tsx: 'var(--bright-blue)',
  rs: 'var(--bright-orange)', py: 'var(--bright-green)',
  go: 'var(--bright-aqua)', json: 'var(--bright-yellow)',
  toml: 'var(--bright-orange)', md: 'var(--bright-blue)',
  css: 'var(--bright-purple)', html: 'var(--bright-red)',
  svg: 'var(--bright-orange)', lock: 'var(--fg4)',
};

export function createProjectScm(project, status) {
  const section = el('div', { class: 'scm-project' });

  // Not a git repo — show init
  if (!status) {
    const initArea = el('div', { class: 'scm-init' });
    initArea.appendChild(el('div', { class: 'scm-init__message' }, 'This folder is not tracked by git.'));
    const initBtn = el('button', { class: 'scm-init__btn' });
    initBtn.appendChild(icon('M12 5v14M5 12h14', 14));
    initBtn.appendChild(el('span', {}, 'Initialize Repository'));
    initBtn.addEventListener('click', () => initRepo(project.id));
    initArea.appendChild(initBtn);
    section.appendChild(initArea);
    return section;
  }

  const branchName = status.branch;

  // ── Commit input (full width) ──
  const commitArea = el('div', { class: 'scm-commit' });
  const commitInput = el('input', {
    class: 'scm-commit__input',
    type: 'text',
    placeholder: `Message (Ctrl+Enter to commit on "${branchName}")`,
    spellcheck: 'false',
  });
  commitInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.ctrlKey) {
      doCommit();
    }
  });
  commitArea.appendChild(commitInput);
  section.appendChild(commitArea);

  // ── Commit button row (full width, with dropdown) ──
  const commitRow = el('div', { class: 'scm-commit-row' });

  const commitBtn = el('button', { class: 'scm-commit-btn' });
  commitBtn.appendChild(icon('M5 12l5 5L20 7', 12));
  commitBtn.appendChild(el('span', {}, 'Commit'));
  commitBtn.addEventListener('click', doCommit);

  // Dropdown chevron for commit options
  const commitDropdownBtn = el('button', { class: 'scm-commit-dropdown' });
  commitDropdownBtn.appendChild(icon('M6 9l6 6 6-6', 10));

  const commitMenu = createDropdownMenu([
    { label: 'Commit', shortcut: 'Ctrl+Enter', action: doCommit },
    { label: 'Commit & Push', action: doCommitAndPush },
    { separator: true },
    { label: 'Push', action: () => pushChanges(project.id).catch(() => {}) },
    { label: 'Pull', action: () => pullChanges(project.id).catch(() => {}) },
    { label: 'Fetch', action: () => fetchChanges(project.id).catch(() => {}) },
  ]);
  document.body.appendChild(commitMenu.element);
  commitDropdownBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    const rect = commitRow.getBoundingClientRect();
    commitMenu.show(rect.left, rect.bottom + 2);
  });

  commitRow.appendChild(commitBtn);
  commitRow.appendChild(commitDropdownBtn);
  section.appendChild(commitRow);

  function doCommit() {
    const msg = commitInput.value.trim();
    if (msg) { commitChanges(project.id, msg); commitInput.value = ''; }
  }
  function doCommitAndPush() {
    const msg = commitInput.value.trim();
    if (msg) { commitAndPush(project.id, msg).catch(() => {}); commitInput.value = ''; }
  }

  // ── Staged changes ──
  const staged = status.files.filter(f => f.is_staged);
  if (staged.length > 0) {
    section.appendChild(createChangeGroup('Staged Changes', staged, project.id, true));
  }

  // ── Unstaged changes ──
  const unstaged = status.files.filter(f => !f.is_staged);
  if (unstaged.length > 0) {
    section.appendChild(createChangeGroup('Changes', unstaged, project.id, false));
  }

  if (status.files.length === 0) {
    section.appendChild(el('div', { class: 'scm-project__empty' }, 'No changes'));
  }

  return section;
}

// ── Collapsible change group ──

function createChangeGroup(title, files, projectId, isStagedGroup) {
  const group = el('div', { class: 'scm-group' });
  let collapsed = false;

  const groupHeader = el('div', { class: 'scm-group__header' });

  const caret = el('span', { class: 'scm-group__caret' });
  caret.appendChild(icon('M6 9l6 6 6-6', 10));

  const titleEl = el('span', { class: 'scm-group__title' }, title);

  const count = el('span', { class: 'scm-group__count' }, String(files.length));

  // Bulk action buttons (visible on hover)
  const actions = el('div', { class: 'scm-group__actions' });

  if (!isStagedGroup) {
    // Stage All
    const stageAllBtn = el('button', { class: 'scm-group__action', title: 'Stage All Changes' });
    stageAllBtn.appendChild(icon('M12 5v14M5 12h14', 12));
    stageAllBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      stageFiles(projectId, files.map(f => f.path));
    });
    actions.appendChild(stageAllBtn);

    // Discard All
    const discardAllBtn = el('button', { class: 'scm-group__action', title: 'Discard All Changes' });
    discardAllBtn.appendChild(icon('M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', 12));
    discardAllBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      const paths = files.filter(f => f.status !== 'Untracked').map(f => f.path);
      if (paths.length > 0) discardChanges(projectId, paths);
    });
    actions.appendChild(discardAllBtn);
  } else {
    // Unstage All
    const unstageAllBtn = el('button', { class: 'scm-group__action', title: 'Unstage All Changes' });
    unstageAllBtn.appendChild(icon('M5 12h14M12 5l-7 7 7 7', 12));
    unstageAllBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      unstageFiles(projectId, files.map(f => f.path));
    });
    actions.appendChild(unstageAllBtn);
  }

  groupHeader.appendChild(caret);
  groupHeader.appendChild(titleEl);
  groupHeader.appendChild(count);
  groupHeader.appendChild(actions);

  const fileList = el('div', { class: 'scm-group__files' });
  for (const file of files) {
    fileList.appendChild(createFileEntry(file, projectId, isStagedGroup));
  }

  // Toggle collapse
  groupHeader.addEventListener('click', () => {
    collapsed = !collapsed;
    fileList.style.display = collapsed ? 'none' : '';
    caret.innerHTML = '';
    caret.appendChild(icon(collapsed ? 'M9 18l6-6-6-6' : 'M6 9l6 6 6-6', 10));
  });

  group.appendChild(groupHeader);
  group.appendChild(fileList);
  return group;
}

// ── File entry (VS Code style) ──

function createFileEntry(file, projectId, isStagedGroup) {
  const entry = el('div', { class: 'scm-file' });

  // File icon (colored by extension)
  const fileName = file.path.split('/').pop() || file.path;
  const ext = fileName.includes('.') ? fileName.split('.').pop().toLowerCase() : '';
  const iconColor = EXT_COLORS[ext] || 'var(--fg4)';

  const fileIcon = el('span', { class: 'scm-file__icon' });
  const svg = icon('M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z', 14);
  svg.style.color = iconColor;
  fileIcon.appendChild(svg);

  // File name
  const nameEl = el('span', { class: 'scm-file__name' }, fileName);

  // Directory path (dimmed)
  const dir = file.path.includes('/') ? file.path.slice(0, file.path.lastIndexOf('/')) : '';
  const dirEl = dir ? el('span', { class: 'scm-file__dir' }, dir) : null;

  // Action buttons (hover)
  const actions = el('div', { class: 'scm-file__actions' });

  if (isStagedGroup) {
    const unstageBtn = createFileAction('Unstage', 'M5 12h14M12 5l-7 7 7 7', () => {
      unstageFiles(projectId, [file.path]);
    });
    actions.appendChild(unstageBtn);
  } else {
    if (file.status !== 'Untracked') {
      const discardBtn = createFileAction('Discard', 'M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', () => {
        discardChanges(projectId, [file.path]);
      });
      actions.appendChild(discardBtn);
    }
    const stageBtn = createFileAction('Stage', 'M12 5v14M5 12h14', () => {
      stageFiles(projectId, [file.path]);
    });
    actions.appendChild(stageBtn);
  }

  // Status letter (far right)
  const statusInfo = STATUS_ICONS[file.status] || { letter: '?', color: 'var(--fg4)' };
  const statusEl = el('span', {
    class: 'scm-file__status',
    style: { color: statusInfo.color },
  }, statusInfo.letter);

  entry.appendChild(fileIcon);
  entry.appendChild(nameEl);
  if (dirEl) entry.appendChild(dirEl);
  entry.appendChild(actions);
  entry.appendChild(statusEl);

  return entry;
}

function createFileAction(title, iconPath, onClick) {
  const btn = el('button', { title });
  btn.appendChild(icon(iconPath, 14));
  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    onClick();
  });
  return btn;
}
