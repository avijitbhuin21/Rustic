import { el, icon } from '../../utils/dom.js';
import { searchStore } from '../../state/search.js';
import { openFile } from '../../state/editor.js';
import { workspaceStore } from '../../state/workspace.js';

export function createSearchResults() {
  const container = el('div', { class: 'search-results' });

  function render() {
    container.innerHTML = '';
    const results = searchStore.getState('results');
    const isSearching = searchStore.getState('isSearching');
    const query = searchStore.getState('query');

    if (isSearching) {
      container.appendChild(el('div', { class: 'search-results__status' }, 'Searching...'));
      return;
    }

    if (!query.trim()) return;

    if (results.length === 0) {
      container.appendChild(el('div', { class: 'search-results__status' }, 'No results found'));
      return;
    }

    // Count total matches
    const totalMatches = results.reduce((sum, r) => sum + r.matches.length, 0);
    container.appendChild(
      el('div', { class: 'search-results__summary' },
        `${totalMatches} result${totalMatches !== 1 ? 's' : ''} in ${results.length} file${results.length !== 1 ? 's' : ''}`)
    );

    for (const result of results) {
      container.appendChild(createFileResult(result));
    }
  }

  searchStore.subscribe('results', render);
  searchStore.subscribe('isSearching', render);

  return container;
}

function createFileResult(result) {
  const section = el('div', { class: 'search-file-result' });

  // Determine project name from path
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

  // File header (collapsible)
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

  header.appendChild(caret);
  header.appendChild(fileIcon);
  header.appendChild(pathLabel);
  header.appendChild(count);

  // Match lines
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
    openFile(filePath, projectName);
    // TODO: scroll to line in Phase 14
  });

  return line;
}
