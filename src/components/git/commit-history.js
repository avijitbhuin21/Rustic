import { el, icon } from '../../utils/dom.js';
import { getCommitFiles } from '../../state/git.js';
import { showContextMenu } from '../dropdown-menu.js';

const FILE_STATUS_COLORS = {
  added: 'var(--bright-green)',
  modified: 'var(--bright-yellow)',
  deleted: 'var(--bright-red)',
  renamed: 'var(--bright-blue)',
  copied: 'var(--bright-aqua)',
};

const FILE_STATUS_LETTERS = {
  added: 'A',
  modified: 'M',
  deleted: 'D',
  renamed: 'R',
  copied: 'C',
};

/**
 * Creates the commit history graph panel for a project.
 */
export function createCommitHistory(projectId, commits, onCommitFileClick) {
  const container = el('div', { class: 'commit-history' });

  if (!commits || commits.length === 0) {
    container.appendChild(el('div', { class: 'commit-history__empty' }, 'No commits yet'));
    return container;
  }

  const list = el('div', { class: 'commit-history__list' });

  for (let i = 0; i < commits.length; i++) {
    const commit = commits[i];
    const isLast = i === commits.length - 1;
    list.appendChild(createCommitEntry(projectId, commit, i, isLast, onCommitFileClick));
  }

  container.appendChild(list);
  return container;
}

function createCommitEntry(projectId, commit, index, isLast, onCommitFileClick) {
  const entry = el('div', { class: 'commit-entry' });

  // Graph line + dot
  const graph = el('div', { class: 'commit-entry__graph' });
  const dot = el('div', { class: 'commit-entry__dot' });
  if (index === 0) dot.classList.add('commit-entry__dot--head');
  const line = el('div', { class: 'commit-entry__line' });
  if (isLast) line.classList.add('commit-entry__line--last');
  graph.appendChild(dot);
  graph.appendChild(line);

  // Content
  const content = el('div', { class: 'commit-entry__content' });

  // Top row: message + refs
  const messageRow = el('div', { class: 'commit-entry__message-row' });

  const firstLine = commit.message.split('\n')[0];
  const msgEl = el('span', { class: 'commit-entry__message' }, firstLine);
  messageRow.appendChild(msgEl);

  if (commit.refs && commit.refs.length > 0) {
    for (const ref of commit.refs) {
      const badge = el('span', { class: 'commit-entry__ref' }, ref);
      messageRow.appendChild(badge);
    }
  }

  content.appendChild(messageRow);

  // Bottom row: author, short hash, relative time
  const metaRow = el('div', { class: 'commit-entry__meta' });
  const authorEl = el('span', { class: 'commit-entry__author' }, commit.author_name);
  const hashEl = el('span', { class: 'commit-entry__hash' }, commit.short_id);
  const timeEl = el('span', { class: 'commit-entry__time' }, formatRelativeTime(commit.timestamp));
  metaRow.appendChild(authorEl);
  metaRow.appendChild(hashEl);
  metaRow.appendChild(timeEl);
  content.appendChild(metaRow);

  // Expandable file list
  const filesContainer = el('div', { class: 'commit-entry__files' });
  filesContainer.style.display = 'none';
  let filesLoaded = false;

  entry.appendChild(graph);
  entry.appendChild(content);
  entry.appendChild(filesContainer);

  // Click to expand/collapse files
  content.addEventListener('click', async () => {
    const isVisible = filesContainer.style.display !== 'none';
    if (isVisible) {
      filesContainer.style.display = 'none';
      entry.classList.remove('commit-entry--expanded');
      return;
    }

    entry.classList.add('commit-entry--expanded');
    filesContainer.style.display = '';

    if (!filesLoaded) {
      filesContainer.innerHTML = '';
      const loadingEl = el('div', { class: 'commit-entry__loading' }, 'Loading...');
      filesContainer.appendChild(loadingEl);

      const files = await getCommitFiles(projectId, commit.oid);
      filesContainer.innerHTML = '';
      filesLoaded = true;

      if (files.length === 0) {
        filesContainer.appendChild(el('div', { class: 'commit-entry__loading' }, 'No file changes'));
        return;
      }

      for (const file of files) {
        filesContainer.appendChild(createCommitFileEntry(projectId, commit.oid, file, onCommitFileClick));
      }
    }
  });

  return entry;
}

function createCommitFileEntry(projectId, oid, file, onCommitFileClick) {
  const entry = el('div', { class: 'commit-file' });

  const statusColor = FILE_STATUS_COLORS[file.status] || 'var(--fg4)';
  const statusLetter = FILE_STATUS_LETTERS[file.status] || '?';

  const fileName = file.path.split('/').pop() || file.path;
  const fileIcon = el('span', { class: 'commit-file__icon' });
  const svg = icon('M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z', 14);
  svg.style.color = 'var(--fg4)';
  fileIcon.appendChild(svg);

  const nameEl = el('span', { class: 'commit-file__name' }, fileName);

  const dir = file.path.includes('/') ? file.path.slice(0, file.path.lastIndexOf('/')) : '';
  const dirEl = dir ? el('span', { class: 'commit-file__dir' }, dir) : null;

  const statsEl = el('span', { class: 'commit-file__stats' });
  if (file.additions > 0) {
    statsEl.appendChild(el('span', { class: 'commit-file__additions' }, `+${file.additions}`));
  }
  if (file.deletions > 0) {
    statsEl.appendChild(el('span', { class: 'commit-file__deletions' }, `-${file.deletions}`));
  }

  const statusEl = el('span', {
    class: 'commit-file__status',
    style: { color: statusColor },
  }, statusLetter);

  // Click opens diff view
  entry.addEventListener('click', (e) => {
    e.stopPropagation();
    if (onCommitFileClick) onCommitFileClick(projectId, file.path, oid);
  });

  // Right-click context menu
  entry.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    showContextMenu([
      { label: 'Open Changes', action: () => {
        if (onCommitFileClick) onCommitFileClick(projectId, file.path, oid);
      }},
      { label: 'Open File', action: () => {
        window.dispatchEvent(new CustomEvent('rustic:open-file', {
          detail: { path: file.path, name: fileName },
        }));
      }},
      { separator: true },
      { label: 'Copy Path', action: () => navigator.clipboard.writeText(file.path) },
    ], e.clientX, e.clientY);
  });

  entry.appendChild(fileIcon);
  entry.appendChild(nameEl);
  if (dirEl) entry.appendChild(dirEl);
  entry.appendChild(statsEl);
  entry.appendChild(statusEl);

  return entry;
}

function formatRelativeTime(timestampSeconds) {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - timestampSeconds;

  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
  if (diff < 2592000) return `${Math.floor(diff / 604800)}w ago`;
  if (diff < 31536000) return `${Math.floor(diff / 2592000)}mo ago`;
  return `${Math.floor(diff / 31536000)}y ago`;
}
