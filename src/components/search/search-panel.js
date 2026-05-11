import { el, icon, iconMulti } from '../../utils/dom.js';
import { searchStore, setQuery, setScope, toggleOption, performSearch, setReplaceText, replaceAll } from '../../state/search.js';
import * as api from '../../lib/tauri-api.js';
import { workspaceStore } from '../../state/workspace.js';
import { createSearchResults } from './search-results.js';

export function createSearchPanel() {
  const panel = el('div', { class: 'search-panel' });

  // --- Scope selector ---
  const scopeSelect = el('select', { class: 'search-scope-select' });

  function updateScopeOptions() {
    scopeSelect.innerHTML = '';
    scopeSelect.appendChild(el('option', { value: 'global' }, 'All Projects'));
    const projects = workspaceStore.getState('projects');
    for (const p of projects) {
      scopeSelect.appendChild(el('option', { value: p.id }, p.name));
    }
    scopeSelect.value = searchStore.getState('scope');
  }

  scopeSelect.addEventListener('change', () => setScope(scopeSelect.value));
  workspaceStore.subscribe('projects', updateScopeOptions);
  updateScopeOptions();

  // --- Search input area ---
  const inputArea = el('div', { class: 'search-input-area' });

  const inputWrapper = el('div', { class: 'search-input-wrapper' });
  const input = el('input', {
    class: 'search-input',
    type: 'text',
    placeholder: 'Search',
    spellcheck: 'false',
  });
  // Hydrate from the store in case the panel is created after a search has
  // already been kicked off (or after the panel was destroyed and recreated
  // in a previous version of this code).
  input.value = searchStore.getState('query') || '';
  input.addEventListener('input', () => setQuery(input.value));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') performSearch();
  });
  inputWrapper.appendChild(input);

  const toggles = el('div', { class: 'search-toggles' });
  toggles.appendChild(createToggleBtn('.*', 'Use Regular Expression', 'isRegex'));
  toggles.appendChild(createToggleBtn('Aa', 'Match Case', 'caseSensitive'));
  toggles.appendChild(createToggleBtn('ab', 'Match Whole Word', 'wholeWord'));
  inputWrapper.appendChild(toggles);
  inputArea.appendChild(inputWrapper);

  // --- Replace row (animated) ---
  const replaceRow = el('div', { class: 'search-replace-row' });

  const replaceWrapper = el('div', { class: 'search-input-wrapper' });
  const replaceInput = el('input', {
    class: 'search-input',
    type: 'text',
    placeholder: 'Replace',
    spellcheck: 'false',
  });
  replaceInput.value = searchStore.getState('replaceText') || '';
  replaceInput.addEventListener('input', () => setReplaceText(replaceInput.value));
  replaceWrapper.appendChild(replaceInput);

  // Replace All icon button (double arrows)
  const replaceAllBtn = el('button', {
    class: 'search-replace-icon-btn',
    title: 'Replace All',
  });
  replaceAllBtn.appendChild(iconMulti([
    'M5 8h14', 'M15 4l4 4-4 4',
    'M5 16h14', 'M15 12l4 4-4 4',
  ], 14));
  replaceAllBtn.addEventListener('click', () => replaceAll());

  replaceRow.appendChild(replaceWrapper);
  replaceRow.appendChild(replaceAllBtn);
  inputArea.appendChild(replaceRow);

  // Expand/collapse toggle for replace row
  let replaceExpanded = false;
  const replaceToggle = el('button', {
    class: 'search-replace-toggle',
    title: 'Toggle Replace',
  });
  replaceToggle.appendChild(icon('M9 6l6 6-6 6', 14));
  replaceToggle.addEventListener('click', () => {
    replaceExpanded = !replaceExpanded;
    replaceRow.classList.toggle('search-replace-row--visible', replaceExpanded);
    replaceToggle.classList.toggle('search-replace-toggle--active', replaceExpanded);
  });
  inputArea.insertBefore(replaceToggle, inputArea.firstChild);

  searchStore.subscribe('isReplacing', (replacing) => {
    replaceAllBtn.disabled = replacing;
  });

  // --- Header with VS Code-style action buttons ---
  const refreshBtn = el('button', { class: 'sidebar-header__action', title: 'Refresh Search' });
  refreshBtn.appendChild(iconMulti([
    'M23 4v6h-6', 'M1 20v-6h6',
    'M3.51 9a9 9 0 0 1 14.85-3.36L23 10',
    'M20.49 15a9 9 0 0 1-14.85 3.36L1 14',
  ], 14));
  refreshBtn.addEventListener('click', async () => {
    refreshBtn.classList.add('spinning');
    const minSpin = new Promise(r => setTimeout(r, 600));
    await Promise.all([performSearch(), minSpin]);
    refreshBtn.classList.remove('spinning');
  });

  const clearBtn = el('button', { class: 'sidebar-header__action', title: 'Clear Search Results' });
  clearBtn.appendChild(iconMulti(['M18 6L6 18', 'M6 6l12 12'], 14));
  clearBtn.addEventListener('click', () => {
    input.value = '';
    replaceInput.value = '';
    // Cancel any in-flight backend walk and bump searchGeneration so the
    // streaming renderer wipes the DOM instead of stranding stale entries.
    api.cancelSearch().catch(() => {});
    const gen = searchStore.getState('searchGeneration');
    searchStore.setState({
      query: '',
      replaceText: '',
      results: [],
      isSearching: false,
      filesScanned: 0,
      filesMatched: 0,
      totalMatches: 0,
      truncated: false,
      currentRootIndex: 0,
      currentRootTotal: 0,
      currentRootName: '',
      searchGeneration: gen + 1,
    });
  });

  const collapseBtn = el('button', { class: 'sidebar-header__action', title: 'Collapse All' });
  collapseBtn.appendChild(iconMulti(['M7 15l5-5 5 5', 'M7 9l5-5 5 5'], 14));
  collapseBtn.addEventListener('click', () => {
    panel.querySelectorAll('.search-file-result__matches').forEach(m => {
      m.style.display = 'none';
    });
    panel.querySelectorAll('.search-file-result__caret').forEach(c => {
      c.innerHTML = '';
      c.appendChild(icon('M9 18l6-6-6-6', 12));
    });
  });

  const headerActions = el('div', { class: 'sidebar-header__actions' }, [
    scopeSelect,
    refreshBtn,
    clearBtn,
    collapseBtn,
  ]);

  const header = el('div', { class: 'sidebar-header' }, [
    el('span', {}, 'Search'),
    headerActions,
  ]);

  // --- Results ---
  const resultsContainer = createSearchResults();

  // --- Assemble panel ---
  panel.appendChild(header);
  panel.appendChild(inputArea);
  panel.appendChild(resultsContainer);

  requestAnimationFrame(() => input.focus());

  return panel;
}

function createToggleBtn(label, title, option) {
  const btn = el('button', {
    class: `search-toggle ${searchStore.getState(option) ? 'search-toggle--active' : ''}`,
    title,
  }, label);

  btn.addEventListener('click', () => toggleOption(option));

  searchStore.subscribe(option, (value) => {
    btn.classList.toggle('search-toggle--active', value);
  });

  return btn;
}
