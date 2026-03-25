import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';

export const searchStore = createStore({
  query: '',
  results: [],          // Array of { file_path, matches: [{ line_number, line_text, match_start, match_end }] }
  isSearching: false,
  scope: 'global',      // 'global' or a project ID
  isRegex: false,
  caseSensitive: false,
  wholeWord: false,
});

let searchTimeout = null;

export async function performSearch() {
  const { query, scope, isRegex, caseSensitive, wholeWord } = searchStore.getState();

  if (!query.trim()) {
    searchStore.setState({ results: [], isSearching: false });
    return;
  }

  searchStore.setState({ isSearching: true });

  try {
    let results;
    if (scope === 'global') {
      results = await api.searchGlobal(query, isRegex, caseSensitive, wholeWord, null, null);
    } else {
      results = await api.searchInProject(scope, query, isRegex, caseSensitive, wholeWord, null, null);
    }
    searchStore.setState({ results: results || [], isSearching: false });
  } catch (e) {
    console.error('Search failed:', e);
    searchStore.setState({ results: [], isSearching: false });
  }
}

export function setQuery(query) {
  searchStore.setState({ query });
  // Debounce search
  clearTimeout(searchTimeout);
  searchTimeout = setTimeout(performSearch, 300);
}

export function setScope(scope) {
  searchStore.setState({ scope });
  performSearch();
}

export function toggleOption(option) {
  const current = searchStore.getState(option);
  searchStore.setState({ [option]: !current });
  performSearch();
}
