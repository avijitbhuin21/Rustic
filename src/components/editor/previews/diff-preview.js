import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * Parse a unified diff string into the same { file_path, hunks, additions, deletions }
 * shape that the git backend returns, so we can render it identically.
 */
function parseUnifiedDiff(text, filePath) {
  const lines = text.split('\n');
  const hunks = [];
  let currentHunk = null;
  let additions = 0;
  let deletions = 0;
  let oldLine = 0;
  let newLine = 0;

  for (const raw of lines) {
    // Hunk header: @@ -old,len +new,len @@
    const hunkMatch = raw.match(/^@@\s+-(\d+)(?:,\d+)?\s+\+(\d+)(?:,\d+)?\s+@@(.*)/);
    if (hunkMatch) {
      currentHunk = { header: raw, lines: [] };
      hunks.push(currentHunk);
      oldLine = parseInt(hunkMatch[1], 10);
      newLine = parseInt(hunkMatch[2], 10);
      continue;
    }

    // Skip diff headers (---, +++, diff --git, index, etc.)
    if (!currentHunk) continue;

    if (raw.startsWith('+')) {
      additions++;
      currentHunk.lines.push({ origin: '+', content: raw.slice(1), old_lineno: null, new_lineno: newLine });
      newLine++;
    } else if (raw.startsWith('-')) {
      deletions++;
      currentHunk.lines.push({ origin: '-', content: raw.slice(1), old_lineno: oldLine, new_lineno: null });
      oldLine++;
    } else if (raw.startsWith(' ') || raw === '') {
      // Context line (or empty trailing line)
      if (currentHunk.lines.length > 0 || raw.startsWith(' ')) {
        currentHunk.lines.push({ origin: ' ', content: raw.startsWith(' ') ? raw.slice(1) : raw, old_lineno: oldLine, new_lineno: newLine });
        oldLine++;
        newLine++;
      }
    }
  }

  return { file_path: filePath, hunks, additions, deletions };
}

export function createDiffPreview() {
  const container = el('div', { class: 'preview-container diff-preview' });

  async function load(diffData) {
    container.innerHTML = '';

    const { projectId, filePath, oid, isStaged, unifiedDiff } = diffData;
    const loading = el('div', { class: 'diff-view__empty' }, 'Loading diff...');
    container.appendChild(loading);

    try {
      let diff;
      if (unifiedDiff) {
        // Pre-computed unified diff from the agent — parse it locally
        diff = parseUnifiedDiff(unifiedDiff, filePath);
      } else if (oid) {
        // Commit diff
        diff = await api.gitCommitFileDiff(projectId, oid, filePath);
      } else if (isStaged) {
        // Staged diff — use gitDiffStaged and find the right file
        const allDiffs = await api.gitDiffStaged(projectId);
        diff = allDiffs?.find(d => d.file_path === filePath);
      } else {
        // Working tree diff
        diff = await api.gitDiff(projectId, filePath);
      }

      container.innerHTML = '';

      if (!diff || !diff.hunks || diff.hunks.length === 0) {
        container.appendChild(el('div', { class: 'diff-view__empty' }, 'No differences'));
        return;
      }

      // Header
      const header = el('div', { class: 'diff-view__header' }, [
        el('span', { class: 'diff-view__path' }, diff.file_path),
        el('span', { class: 'diff-view__stats' }, [
          el('span', { class: 'diff-view__additions' }, `+${diff.additions}`),
          el('span', { class: 'diff-view__deletions' }, `-${diff.deletions}`),
        ]),
      ]);
      container.appendChild(header);

      // Hunks
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
      container.innerHTML = '';
      container.appendChild(el('div', { class: 'diff-view__empty' }, `Error loading diff: ${e}`));
    }
  }

  function destroy() {
    container.innerHTML = '';
  }

  return { element: container, load, destroy };
}
