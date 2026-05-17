import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';
import { createCollapsible } from './settings-controls.js';

/**
 * Settings → Logs panel.
 *
 * Lists the rotating log files written by the Rust side, newest first. The
 * top entry (no date suffix) is today's active file; the rest are previous
 * days, retained for 7 days. Clicking a row reads the file via Tauri and
 * opens it as a read-only scratch buffer in the editor so the user can
 * scroll, search, and copy lines without touching the on-disk content.
 *
 * Why scratch buffer instead of `open_file`: the rolling appender is still
 * appending to today's file as the user views it, so a real editor buffer
 * would either show stale content or fight with the writer thread. Scratch
 * buffers are decoupled snapshots — the user re-clicks to refresh.
 */
export function createLogsSettings() {
  const container = el('div', { class: 'settings-section' });

  const headerActions = el('div', { class: 'settings-logs__header-actions' });

  const refreshBtn = el('button', {
    class: 'settings-button',
    type: 'button',
    title: 'Re-scan the logs directory',
  });
  refreshBtn.appendChild(icon('M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15', 14));
  refreshBtn.appendChild(el('span', {}, 'Refresh'));
  headerActions.appendChild(refreshBtn);

  const revealBtn = el('button', {
    class: 'settings-button',
    type: 'button',
    title: 'Open the logs folder in your file manager',
  });
  revealBtn.appendChild(icon('M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z', 14));
  revealBtn.appendChild(el('span', {}, 'Reveal folder'));
  headerActions.appendChild(revealBtn);

  const pathRow = el('div', { class: 'settings-logs__path-row' });
  const pathLabel = el('div', { class: 'settings-logs__path-label' }, 'Logs directory');
  const pathValue = el('code', { class: 'settings-logs__path-value' }, 'Loading…');
  pathRow.appendChild(pathLabel);
  pathRow.appendChild(pathValue);

  const listBody = el('div', { class: 'settings-logs__list' });
  const empty = el('div', { class: 'settings-logs__empty' },
    'No logs yet. They\'ll appear here after the app has been running.');
  const loading = el('div', { class: 'settings-logs__loading' }, 'Reading logs directory…');

  const listSection = el('div', { class: 'settings-logs' });
  listSection.appendChild(pathRow);
  listSection.appendChild(listBody);

  container.appendChild(createCollapsible('Application logs', listSection, true, headerActions));

  // Note about retention so the user knows old logs auto-disappear.
  const note = el('div', { class: 'settings-logs__note' });
  note.appendChild(icon('M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z', 14));
  note.appendChild(el('span', {}, 'Logs rotate daily and are kept for 7 days. Older files are deleted automatically.'));
  container.appendChild(note);

  let dirPath = null;

  async function refresh() {
    listBody.innerHTML = '';
    listBody.appendChild(loading);

    try {
      if (!dirPath) {
        try {
          dirPath = await api.getLogsDir();
          pathValue.textContent = dirPath || 'Unavailable';
        } catch (e) {
          pathValue.textContent = 'Unavailable';
        }
      }

      const files = await api.listLogFiles();
      listBody.innerHTML = '';

      if (!files || files.length === 0) {
        listBody.appendChild(empty);
        return;
      }

      for (const f of files) {
        listBody.appendChild(renderRow(f));
      }
    } catch (e) {
      listBody.innerHTML = '';
      const err = el('div', { class: 'settings-logs__error' },
        `Failed to read logs: ${e?.message ?? e}`);
      listBody.appendChild(err);
    }
  }

  function renderRow(f) {
    const row = el('div', { class: 'settings-logs__row' });

    const left = el('div', { class: 'settings-logs__row-left' });
    const dateLabel = f.date
      ? formatDate(f.date)
      : 'Today (active)';
    left.appendChild(el('div', { class: 'settings-logs__row-date' }, dateLabel));
    left.appendChild(el('div', { class: 'settings-logs__row-name' }, f.name));
    row.appendChild(left);

    const size = el('div', { class: 'settings-logs__row-size' }, formatSize(f.size_bytes));
    row.appendChild(size);

    const openBtn = el('button', {
      class: 'settings-button settings-button--primary',
      type: 'button',
      title: 'Open this log in the editor',
    });
    openBtn.appendChild(icon('M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z', 14));
    openBtn.appendChild(el('span', {}, 'Open'));
    openBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      openBtn.disabled = true;
      try {
        await openLogInEditor(f);
      } catch (err) {
        console.error('[logs] failed to open log:', err);
      } finally {
        openBtn.disabled = false;
      }
    });
    row.appendChild(openBtn);

    // Whole-row click also opens, so the click target is forgiving.
    row.addEventListener('click', () => openBtn.click());

    return row;
  }

  refreshBtn.addEventListener('click', () => refresh());
  revealBtn.addEventListener('click', async () => {
    if (!dirPath) return;
    try {
      await api.revealInFileManager(dirPath);
    } catch (e) {
      console.error('[logs] revealInFileManager failed:', e);
    }
  });

  // Initial fetch
  refresh();

  return container;
}

/**
 * Read the file via the scoped backend command and open it as a scratch
 * buffer in the editor. Inlined rather than importing chat-view.js's helper
 * so settings don't pull in the entire chat module.
 */
async function openLogInEditor(f) {
  const content = await api.readLogFile(f.path);
  const title = f.date ? `Log ${f.date}` : f.name;
  const info = await api.openScratchBuffer(title, content ?? '', 'log');
  if (!info) return;

  const { editorStore, setActiveBuffer } = await import('../../state/editor.js');
  const buffer = {
    id: info.id,
    filePath: info.file_path,
    fileName: info.file_name,
    projectName: '',
    lineCount: info.line_count,
    language: info.language,
    isModified: false,
    fileType: 'code',
    isPreview: false,
    isDualMode: false,
    viewMode: 'edit',
  };
  const newBuffers = { ...editorStore.getState('openBuffers'), [info.id]: buffer };
  editorStore.setState({ openBuffers: newBuffers });
  // Settings is itself just an active buffer (`fileType: 'settings'`).
  // Switching to the new scratch buffer auto-hides the settings panel via
  // editor-group.js's visibility subscriber — no explicit close needed.
  setActiveBuffer(info.id);
}

function formatDate(yyyy_mm_dd) {
  // Render as "Mon, 5 May 2026" — short month name, locale-friendly.
  const d = new Date(yyyy_mm_dd + 'T00:00:00');
  if (isNaN(d.getTime())) return yyyy_mm_dd;
  return d.toLocaleDateString(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    year: 'numeric',
  });
}

function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}
