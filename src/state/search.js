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
  // Streaming-search progress shown in the summary line while a walk is running.
  filesScanned: 0,
  filesMatched: 0,
  totalMatches: 0,
  truncated: false,
  // Per-project progress. In global-scope searches the backend walks projects
  // sequentially and emits rootStarted/rootCompleted around each. We surface
  // these so the summary can show "Searching [project name] (2 of 5)".
  currentRootIndex: 0,    // 0-based index of the project currently being scanned
  currentRootTotal: 0,    // total projects in the scope (0 when not searching)
  currentRootName: '',
  // Bumped every time we start a new search. The renderer compares this with
  // its last-rendered generation to decide between full-redraw and append-only.
  searchGeneration: 0,
});

let searchTimeout = null;
// Search id returned by the backend for the currently-active search. Every
// `search-event` payload carries the id it was emitted for; we drop anything
// that doesn't match, which makes stale results from superseded searches
// impossible to leak through.
let currentSearchId = null;
let eventListenerInstalled = false;

async function ensureEventListener() {
  if (eventListenerInstalled) return;
  eventListenerInstalled = true;
  await api.onSearchEvent((payload) => {
    if (!payload || payload.search_id !== currentSearchId) return;
    switch (payload.kind) {
      case 'fileMatch': {
        const state = searchStore.getState();
        searchStore.setState({
          results: [...state.results, payload.result],
          filesMatched: state.filesMatched + 1,
          totalMatches: state.totalMatches + payload.result.matches.length,
        });
        break;
      }
      case 'progress': {
        // Progress only updates the scanned counter; matched/total counts are
        // already accurate from fileMatch events and authoritative completed.
        searchStore.setState({
          filesScanned: payload.accumulated_files_scanned,
        });
        break;
      }
      case 'rootStarted': {
        searchStore.setState({
          currentRootIndex: payload.index,
          currentRootTotal: payload.total,
          currentRootName: payload.project_name,
        });
        break;
      }
      case 'rootCompleted': {
        // No-op for now — running counts are already kept up to date by
        // fileMatch events. Kept as a hook in case we later want per-project
        // dividers in the results list.
        break;
      }
      case 'completed': {
        searchStore.setState({
          isSearching: false,
          filesScanned: payload.files_scanned,
          filesMatched: payload.files_matched,
          totalMatches: payload.total_matches,
          truncated: payload.truncated,
          currentRootTotal: 0,
          currentRootName: '',
        });
        break;
      }
    }
  });
}

export async function performSearch() {
  const { query, scope, isRegex, caseSensitive, wholeWord, searchGeneration } = searchStore.getState();

  await ensureEventListener();

  if (!query.trim()) {
    // Tell backend to drop any in-flight walk and reset our local view.
    try { await api.cancelSearch(); } catch {}
    currentSearchId = null;
    searchStore.setState({
      results: [],
      isSearching: false,
      filesScanned: 0,
      filesMatched: 0,
      totalMatches: 0,
      truncated: false,
      currentRootIndex: 0,
      currentRootTotal: 0,
      currentRootName: '',
      searchGeneration: searchGeneration + 1,
    });
    return;
  }

  // Reset visible state immediately so the user sees "Searching..." right
  // away instead of stale results from the previous query.
  searchStore.setState({
    results: [],
    isSearching: true,
    filesScanned: 0,
    filesMatched: 0,
    totalMatches: 0,
    truncated: false,
    currentRootIndex: 0,
    currentRootTotal: 0,
    currentRootName: '',
    searchGeneration: searchGeneration + 1,
  });

  try {
    const id = await api.startSearch(scope, query, isRegex, caseSensitive, wholeWord, null, null);
    currentSearchId = id;
  } catch (e) {
    console.error('Search failed to start:', e);
    searchStore.setState({ isSearching: false });
  }
}

export function setQuery(query) {
  searchStore.setState({ query });
  // Debounce search. 350ms is a touch longer than the typical inter-keystroke
  // interval, so a burst of typing collapses into one search instead of N.
  clearTimeout(searchTimeout);
  searchTimeout = setTimeout(performSearch, 350);
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
