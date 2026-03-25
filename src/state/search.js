import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';

export const searchStore = createStore({
  query: '',
  replaceText: '',
  results: [],          // Array of { file_path, matches: [{ line_number, line_text, match_start, match_end }] }
  isSearching: false,
  isReplacing: false,
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

export function setReplaceText(text) {
  searchStore.setState({ replaceText: text });
}

export async function replaceInSingleFile(filePath) {
  const { query, replaceText, isRegex, caseSensitive, wholeWord } = searchStore.getState();
  if (!query.trim()) return;

  searchStore.setState({ isReplacing: true });
  try {
    await api.replaceInFile(filePath, query, replaceText, isRegex, caseSensitive, wholeWord);
    // Re-run search to update results
    await performSearch();
    // If file is open in editor, reload it
    const { editorStore } = await import('./editor.js');
    const buffers = editorStore.getState('openBuffers');
    for (const buf of Object.values(buffers)) {
      if (buf.filePath === filePath && !buf.isPreview) {
        // Reopen to pick up disk changes — close and reopen
        const { closeBuffer, openFile } = await import('./editor.js');
        const projectName = buf.projectName;
        await closeBuffer(buf.id, { force: true });
        await openFile(filePath, projectName);
        break;
      }
    }
  } catch (e) {
    console.error('Replace failed:', e);
  }
  searchStore.setState({ isReplacing: false });
}

export async function replaceAll() {
  const { query, replaceText, results, isRegex, caseSensitive, wholeWord } = searchStore.getState();
  if (!query.trim() || results.length === 0) return;

  searchStore.setState({ isReplacing: true });
  try {
    const filePaths = results.map(r => r.file_path);
    for (const filePath of filePaths) {
      await api.replaceInFile(filePath, query, replaceText, isRegex, caseSensitive, wholeWord);
    }

    // Reload any open buffers whose files were changed
    const { editorStore } = await import('./editor.js');
    const { closeBuffer, openFile } = await import('./editor.js');
    const buffers = editorStore.getState('openBuffers');
    const fileSet = new Set(filePaths);
    for (const buf of Object.values(buffers)) {
      if (fileSet.has(buf.filePath) && !buf.isPreview) {
        const projectName = buf.projectName;
        await closeBuffer(buf.id, { force: true });
        await openFile(buf.filePath, projectName);
      }
    }

    // Re-run search to update results
    await performSearch();
  } catch (e) {
    console.error('Replace all failed:', e);
  }
  searchStore.setState({ isReplacing: false });
}
