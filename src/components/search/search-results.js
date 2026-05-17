import { el, icon } from '../../utils/dom.js';
import { searchStore, replaceInSingleFile } from '../../state/search.js';
import { openFileAtLine } from '../../state/editor.js';
import { workspaceStore } from '../../state/workspace.js';

export function createSearchResults() {
  const container = el('div', { class: 'search-results' });

  // Append-only renderer state. `searchGeneration` bumps every time a new
  // query starts; when it changes we wipe the DOM and start fresh. Within the
  // same generation we only append new file results, so a 1000-file streaming
  // search produces 1000 tiny appends instead of 1000 full redraws.
  let renderedGeneration = -1;
  let renderedCount = 0;
  let summaryEl = null;
  let listEl = null;
  let emptyStatusEl = null;

  function formatSummary() {
    const state = searchStore.getState();
    const totalMatches = state.totalMatches;
    const filesMatched = state.filesMatched;
    const scanned = state.filesScanned;
    const head = `${totalMatches} result${totalMatches !== 1 ? 's' : ''} in ${filesMatched} file${filesMatched !== 1 ? 's' : ''}`;
    if (state.isSearching) {
      // Show "Searching [project] (i of N)" when walking multiple projects so
      // the user can see we're progressing sequentially instead of stalled.
      let scope = '';
      if (state.currentRootTotal > 1 && state.currentRootName) {
        scope = ` — [${state.currentRootName}] (${state.currentRootIndex + 1} of ${state.currentRootTotal})`;
      } else if (state.currentRootName) {
        scope = ` — [${state.currentRootName}]`;
      }
      return `${head}${scope} · scanned ${scanned} file${scanned !== 1 ? 's' : ''}…`;
    }
    if (state.truncated) {
      return `${head} (truncated — narrow your search for more)`;
    }
    return head;
  }

  function fullRedraw() {
    container.innerHTML = '';
    summaryEl = null;
    listEl = null;
    emptyStatusEl = null;

    const state = searchStore.getState();
    if (!state.query.trim()) {
      renderedCount = 0;
      return;
    }

    summaryEl = el('div', { class: 'search-results__summary' }, formatSummary());
    container.appendChild(summaryEl);

    listEl = el('div', { class: 'search-results__list' });
    container.appendChild(listEl);

    // Show an empty-state node while a search is mid-flight with zero matches
    // so the user gets immediate feedback. Removed on first file match.
    if (state.results.length === 0) {
      const msg = state.isSearching ? 'Searching…' : 'No results found';
      emptyStatusEl = el('div', { class: 'search-results__status' }, msg);
      container.appendChild(emptyStatusEl);
    }

    // Render whatever results are already in the store (typically zero for a
    // fresh search; non-zero if generation bumped but results came in fast).
    renderedCount = 0;
    appendNewResults(state.results);
  }

  function appendNewResults(results) {
    if (!listEl) return;
    if (results.length <= renderedCount) return;

    // Drop the empty-state status if we now have results.
    if (emptyStatusEl && results.length > 0) {
      emptyStatusEl.remove();
      emptyStatusEl = null;
    }

    const frag = document.createDocumentFragment();
    for (let i = renderedCount; i < results.length; i++) {
      frag.appendChild(createFileResult(results[i]));
    }
    listEl.appendChild(frag);
    renderedCount = results.length;
  }

  function syncSummaryText() {
    if (summaryEl) summaryEl.textContent = formatSummary();
  }

  function syncEmptyStatus() {
    const state = searchStore.getState();
    if (!emptyStatusEl) return;
    emptyStatusEl.textContent = state.isSearching ? 'Searching…' : 'No results found';
  }

  function onStoreChange() {
    const state = searchStore.getState();
    if (state.searchGeneration !== renderedGeneration) {
      renderedGeneration = state.searchGeneration;
      fullRedraw();
      return;
    }
    appendNewResults(state.results);
    syncSummaryText();
    syncEmptyStatus();
  }

  // Coalesce the 7 summary-counter subscriptions into one rAF to avoid N DOM updates per setState.
  let summaryRaf = 0;
  function scheduleSummary() {
    if (summaryRaf) return;
    summaryRaf = requestAnimationFrame(() => {
      summaryRaf = 0;
      syncSummaryText();
    });
  }
  searchStore.subscribe('searchGeneration', onStoreChange);
  searchStore.subscribe('results', onStoreChange);
  searchStore.subscribe('isSearching', onStoreChange);
  searchStore.subscribe('filesScanned', scheduleSummary);
  searchStore.subscribe('filesMatched', scheduleSummary);
  searchStore.subscribe('totalMatches', scheduleSummary);
  searchStore.subscribe('truncated', scheduleSummary);
  searchStore.subscribe('currentRootIndex', scheduleSummary);
  searchStore.subscribe('currentRootTotal', scheduleSummary);
  searchStore.subscribe('currentRootName', scheduleSummary);

  // Single subscriber for replaceText updates — avoids N callbacks for an N-file result set.
  searchStore.subscribe('replaceText', () => {
    if (!listEl) return;
    const show = !!searchStore.getState('replaceText');
    const value = show ? 'inline-block' : 'none';
    const btns = listEl.querySelectorAll('.search-file-result__replace-btn');
    for (const btn of btns) btn.style.display = value;
  });

  onStoreChange();

  return container;
}

function createFileResult(result) {
  const section = el('div', { class: 'search-file-result' });

  const projects = workspaceStore.getState('projects');
  let projectName = '';
  let displayPath = result.file_path;
  for (const p of projects) {
    if (result.file_path.startsWith(p.root_path)) {
      projectName = p.name;
      displayPath = result.file_path.slice(p.root_path.length + 1);
      break;
    }
  }

  const header = el('div', { class: 'search-file-result__header' });
  const caret = el('span', { class: 'search-file-result__caret' });
  caret.appendChild(icon('M6 9l6 6 6-6', 12));

  const fileIcon = el('span', { class: 'search-file-result__icon' });
  fileIcon.appendChild(icon('M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z', 14));

  const pathLabel = el('span', { class: 'search-file-result__path' });
  if (projectName) {
    pathLabel.appendChild(el('span', { class: 'search-file-result__project' }, `[${projectName}] `));
  }
  pathLabel.appendChild(document.createTextNode(displayPath));

  const count = el('span', { class: 'search-file-result__count' }, String(result.matches.length));

  const replaceFileBtn = el('button', {
    class: 'search-file-result__replace-btn',
    title: 'Replace all in this file',
  }, 'Replace');
  replaceFileBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    replaceInSingleFile(result.file_path);
  });

  replaceFileBtn.style.display = searchStore.getState('replaceText') ? 'inline-block' : 'none';

  header.appendChild(caret);
  header.appendChild(fileIcon);
  header.appendChild(pathLabel);
  header.appendChild(count);
  header.appendChild(replaceFileBtn);

  const matchList = el('div', { class: 'search-file-result__matches' });
  for (const match of result.matches) {
    const matchEl = createMatchLine(match, result.file_path, projectName);
    matchList.appendChild(matchEl);
  }

  let expanded = true;
  header.addEventListener('click', () => {
    expanded = !expanded;
    matchList.style.display = expanded ? 'block' : 'none';
    caret.innerHTML = '';
    caret.appendChild(icon(expanded ? 'M6 9l6 6 6-6' : 'M9 18l6-6-6-6', 12));
  });

  section.appendChild(header);
  section.appendChild(matchList);

  return section;
}

function createMatchLine(match, filePath, projectName) {
  const line = el('div', { class: 'search-match-line' });

  const lineNum = el('span', { class: 'search-match-line__number' }, String(match.line_number));

  // Build text with highlighted match
  const text = el('span', { class: 'search-match-line__text' });
  const before = match.line_text.slice(0, match.match_start);
  const matched = match.line_text.slice(match.match_start, match.match_end);
  const after = match.line_text.slice(match.match_end);

  if (before) text.appendChild(document.createTextNode(before));
  text.appendChild(el('span', { class: 'search-match-highlight' }, matched));
  if (after) text.appendChild(document.createTextNode(after));

  line.appendChild(lineNum);
  line.appendChild(text);

  // Click to open file at line
  line.addEventListener('click', () => {
    openFileAtLine(filePath, projectName, match.line_number, match.match_start);
  });

  return line;
}
