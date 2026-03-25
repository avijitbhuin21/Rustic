import { el } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

/**
 * Create a read-only inline diff view for a file.
 * Returns a DOM element showing the diff with red/green highlighting.
 */
export async function createDiffView(projectId, filePath) {
  const container = el('div', { class: 'diff-view' });

  try {
    const diff = await api.gitDiff(projectId, filePath);

    if (!diff || diff.hunks.length === 0) {
      container.appendChild(el('div', { class: 'diff-view__empty' }, 'No differences'));
      return container;
    }

    const header = el('div', { class: 'diff-view__header' }, [
      el('span', { class: 'diff-view__path' }, diff.file_path),
      el('span', { class: 'diff-view__stats' }, [
        el('span', { class: 'diff-view__additions' }, `+${diff.additions}`),
        el('span', { class: 'diff-view__deletions' }, `-${diff.deletions}`),
      ]),
    ]);
    container.appendChild(header);

    for (const hunk of diff.hunks) {
      const hunkEl = el('div', { class: 'diff-hunk' });
      hunkEl.appendChild(el('div', { class: 'diff-hunk__header' }, hunk.header));

      for (const line of hunk.lines) {
        let className = 'diff-line';
        if (line.origin === '+') className += ' diff-line--added';
        else if (line.origin === '-') className += ' diff-line--removed';

        const lineEl = el('div', { class: className });

        const gutter = el('span', { class: 'diff-line__gutter' });
        gutter.textContent = line.origin === ' ' ? ' ' : line.origin;

        const lineNum = el('span', { class: 'diff-line__number' });
        if (line.origin === '-') {
          lineNum.textContent = line.old_lineno != null ? String(line.old_lineno) : '';
        } else if (line.origin === '+') {
          lineNum.textContent = line.new_lineno != null ? String(line.new_lineno) : '';
        } else {
          lineNum.textContent = line.old_lineno != null ? String(line.old_lineno) : '';
        }

        const content = el('span', { class: 'diff-line__content' }, line.content);

        lineEl.appendChild(gutter);
        lineEl.appendChild(lineNum);
        lineEl.appendChild(content);
        hunkEl.appendChild(lineEl);
      }

      container.appendChild(hunkEl);
    }
  } catch (e) {
    container.appendChild(el('div', { class: 'diff-view__empty' }, `Error loading diff: ${e}`));
  }

  return container;
}
