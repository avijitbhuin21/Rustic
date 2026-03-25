import { el, icon } from '../../utils/dom.js';
import { searchStore, setQuery, setScope, toggleOption, performSearch } from '../../state/search.js';
import { workspaceStore } from '../../state/workspace.js';
import { createSearchResults } from './search-results.js';

export function createSearchPanel() {
  const panel = el('div', { class: 'search-panel' });

  // Scope selector (in header)
  const scopeSelect = el('select', { class: 'search-scope-select' });

  function updateScopeOptions() {
    scopeSelect.innerHTML = '';
    const globalOpt = el('option', { value: 'global' }, 'All Projects');
    scopeSelect.appendChild(globalOpt);

    const projects = workspaceStore.getState('projects');
    for (const p of projects) {
      const opt = el('option', { value: p.id }, p.name);
      scopeSelect.appendChild(opt);
    }
    scopeSelect.value = searchStore.getState('scope');
  }

  scopeSelect.addEventListener('change', () => setScope(scopeSelect.value));
  workspaceStore.subscribe('projects', updateScopeOptions);
  updateScopeOptions();

  // Header
  const header = el('div', { class: 'sidebar-header' }, [
    el('span', {}, 'Search'),
    scopeSelect,
  ]);

  // Search input area
  const inputArea = el('div', { class: 'search-input-area' });

  const inputWrapper = el('div', { class: 'search-input-wrapper' });
  const input = el('input', {
    class: 'search-input',
    type: 'text',
    placeholder: 'Search...',
    spellcheck: 'false',
  });
  input.addEventListener('input', () => setQuery(input.value));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') performSearch();
  });
  inputWrapper.appendChild(input);

  // Toggle buttons (regex, case, whole word)
  const toggles = el('div', { class: 'search-toggles' });

  const regexBtn = createToggleBtn('.*', 'Use Regular Expression', 'isRegex');
  const caseBtn = createToggleBtn('Aa', 'Match Case', 'caseSensitive');
  const wordBtn = createToggleBtn('ab', 'Match Whole Word', 'wholeWord');

  toggles.appendChild(regexBtn);
  toggles.appendChild(caseBtn);
  toggles.appendChild(wordBtn);

  inputWrapper.appendChild(toggles);
  inputArea.appendChild(inputWrapper);


  // Results
  const resultsContainer = createSearchResults();

  panel.appendChild(header);
  panel.appendChild(inputArea);
  panel.appendChild(resultsContainer);

  // Focus input when panel is shown
  requestAnimationFrame(() => input.focus());

  return panel;
}

function createToggleBtn(label, title, option) {
  const btn = el('button', {
    class: `search-toggle ${searchStore.getState(option) ? 'search-toggle--active' : ''}`,
    title,
  }, label);

  btn.addEventListener('click', () => {
    toggleOption(option);
  });

  searchStore.subscribe(option, (value) => {
    btn.classList.toggle('search-toggle--active', value);
  });

  return btn;
}
