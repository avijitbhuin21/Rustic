import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

let unsubEvent = null;
let activeSearchId = null;

// ---------------------------------------------------------------------------
// Two-tier flush strategy:
//
//  FAST (150 ms)  — updates totalMatches + filesMatched only. Two numbers in
//                   a setState, no DOM work. Drives the live "N matches in M
//                   files" counter.
//
//  SLOW (2 000 ms) — snapshots the full pendingResults buffer into Zustand.
//                   SearchResults re-renders at most once every 2 s during a
//                   long search. If the search finishes before 2 s the commit
//                   fires immediately via commitAndStop().
// ---------------------------------------------------------------------------
const COUNT_FLUSH_MS   = 150;
const RESULTS_FLUSH_MS = 2000;

let pendingResults  = new Map();
let totalMatchesBuf = 0;
let filesMatchedBuf = 0;
let countTimer      = null;
let resultsTimer    = null;

function resetBuffer() {
  if (countTimer   !== null) { clearTimeout(countTimer);   countTimer   = null; }
  if (resultsTimer !== null) { clearTimeout(resultsTimer); resultsTimer = null; }
  pendingResults  = new Map();
  totalMatchesBuf = 0;
  filesMatchedBuf = 0;
}

function flushCountsOnly() {
  countTimer = null;
  useSearch.setState({ totalMatches: totalMatchesBuf, filesMatched: filesMatchedBuf });
}

function flushResultsSnapshot() {
  resultsTimer = null;
  if (countTimer !== null) { clearTimeout(countTimer); countTimer = null; }
  const snapshot = new Map(pendingResults);
  useSearch.setState({ results: snapshot, totalMatches: totalMatchesBuf, filesMatched: filesMatchedBuf });
}

function scheduleCountFlush() {
  if (countTimer === null) countTimer = setTimeout(flushCountsOnly, COUNT_FLUSH_MS);
}

function scheduleResultsFlush() {
  if (resultsTimer === null) resultsTimer = setTimeout(flushResultsSnapshot, RESULTS_FLUSH_MS);
}

function commitAndStop() {
  if (countTimer   !== null) { clearTimeout(countTimer);   countTimer   = null; }
  if (resultsTimer !== null) { clearTimeout(resultsTimer); resultsTimer = null; }
  const results      = pendingResults;
  const totalMatches = totalMatchesBuf;
  const filesMatched = filesMatchedBuf;
  pendingResults  = new Map();
  totalMatchesBuf = 0;
  filesMatchedBuf = 0;
  useSearch.setState({ results, totalMatches, filesMatched, running: false });
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------
export const useSearch = create((set, get) => ({
  query: '',
  replace: '',
  regex: false,
  caseSensitive: false,
  wholeWord: false,
  includeGlobs: '',
  excludeGlobs: '',
  results: new Map(),
  running: false,
  totalMatches: 0,
  filesMatched: 0,
  // Array of project IDs to search in. Persisted so it survives panel remounts.
  // Initialised to [] and set to [first project] by SearchPanel on mount.
  scopeIds: [],

  setField: (k, v) => set({ [k]: v }),
  setScopeIds: (ids) => set({ scopeIds: ids }),

  ensureListener: async () => {
    if (unsubEvent) return;
    unsubEvent = await listen('search-event', (e) => {
      const payload = e.payload ?? {};
      const { kind } = payload;

      if (kind === 'fileMatch') {
        const { search_id, results } = payload;
        if (search_id !== activeSearchId) return;
        if (!Array.isArray(results) || results.length === 0) return;

        for (const result of results) {
          const { file_path, matches } = result ?? {};
          if (!file_path || !Array.isArray(matches) || matches.length === 0) continue;

          const newMatches = matches.map((m) => ({
            line:      m.line_number,
            line_text: m.line_text,
            start:     m.match_start,
            end:       m.match_end,
          }));

          const existing = pendingResults.get(file_path);
          pendingResults.set(file_path, existing ? [...existing, ...newMatches] : newMatches);
          totalMatchesBuf += newMatches.length;
          if (!existing) filesMatchedBuf += 1;
        }

        scheduleCountFlush();
        scheduleResultsFlush();

      } else if (kind === 'completed') {
        if (payload.search_id !== activeSearchId) return;
        commitAndStop();
      }
    });
  },

  start: async () => {
    await get().ensureListener();
    const s = get();
    if (!s.query.trim() || s.scopeIds.length === 0) return;
    if (s.running && activeSearchId != null) {
      try { await invoke('cancel_search'); } catch {}
    }
    resetBuffer();
    set({ results: new Map(), totalMatches: 0, filesMatched: 0, running: true });
    try {
      activeSearchId = await invoke('start_search', {
        scopes: s.scopeIds,
        pattern: s.query,
        isRegex: s.regex,
        caseSensitive: s.caseSensitive,
        wholeWord: s.wholeWord,
        includeGlob: s.includeGlobs.trim() || null,
        excludeGlob: s.excludeGlobs.trim() || null,
      });
    } catch (err) {
      set({ running: false });
      console.error('start_search failed:', err);
    }
  },

  cancel: async () => {
    resetBuffer();
    if (activeSearchId != null) {
      try { await invoke('cancel_search'); } catch {}
    }
    set({ running: false, results: new Map(), totalMatches: 0, filesMatched: 0 });
  },

  replaceInFile: async (path, pattern, replacement, opts = {}) => {
    await invoke('replace_in_file', {
      path,
      pattern,
      replacement,
      isRegex: !!opts.isRegex,
      caseSensitive: !!opts.caseSensitive,
      wholeWord: !!opts.wholeWord,
    });
  },
}));
