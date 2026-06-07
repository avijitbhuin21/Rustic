import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

// Per-project in-flight guard for refreshAll.
// Prevents concurrent calls from stacking up (e.g. rapid user actions).
// Pattern: 'running' = one call active; 'queued' = one more needed after current finishes.
const refreshLocks = new Map();

const emptyStatus = { unstaged: [], staged: [], untracked: [] };
const emptyAheadBehind = { ahead: 0, behind: 0 };
const emptyStatusCounts = { staged: 0, unstaged: 0, untracked: 0 };

// How many file rows to fetch per status page. The backend caps `git_status` to
// this many entries (tracked changes first, untracked tail truncated) and
// reports the TRUE totals separately, so a repo with 80k changed files never
// ships or renders them all at once. "Load more" grows the per-project limit.
// Keep in sync with SCM_PAGE_SIZE in scm-panel.jsx.
const STATUS_PAGE_SIZE = 500;

// Pull the full per-category totals out of a raw git_status payload, falling
// back to array lengths for the already-transformed shape.
function countsFromRaw(raw) {
  if (!raw) return emptyStatusCounts;
  return {
    staged: raw.staged_count ?? (Array.isArray(raw.staged) ? raw.staged.length : 0),
    unstaged: raw.unstaged_count ?? (Array.isArray(raw.unstaged) ? raw.unstaged.length : 0),
    untracked: raw.untracked_count ?? (Array.isArray(raw.untracked) ? raw.untracked.length : 0),
  };
}

// Backend returns { branch, files: [{path, status: "Modified"|"New"|..., is_staged: bool}] }
// Transform to the { staged, unstaged, untracked } shape the UI expects.
const STATUS_CODE = {
  New: 'A',
  Modified: 'M',
  Deleted: 'D',
  Renamed: 'R',
  Untracked: '?',
  Conflicted: 'U',
};

function transformStatus(raw) {
  if (!raw) return emptyStatus;
  // Already in the expected shape (future-proofing)
  if (Array.isArray(raw.staged)) return raw;

  const staged = [];
  const unstaged = [];
  const untracked = [];

  for (const f of raw.files ?? []) {
    const status = STATUS_CODE[f.status] ?? 'M';
    const entry = { path: f.path, file: f.path, status };
    if (f.status === 'Untracked') {
      untracked.push(entry);
    } else if (f.is_staged) {
      staged.push(entry);
    } else {
      unstaged.push(entry);
    }
  }

  return { staged, unstaged, untracked };
}

// Stable empty references — selectors that fall back to these (`?? EMPTY_ARRAY`)
// keep their snapshot identity stable across renders. Returning a fresh `[]`
// each call would trip React's "getSnapshot should be cached" guard inside
// useSyncExternalStore and risk an infinite loop in StrictMode.
export const EMPTY_ARRAY = Object.freeze([]);

const emptyProjectState = () => ({
  status: emptyStatus,
  // True per-category totals (the backend caps `status.*` to statusLimit rows).
  statusCounts: emptyStatusCounts,
  statusLimit: STATUS_PAGE_SIZE,
  statusTruncated: false,
  branches: [],
  currentBranch: null,
  log: [],
  aheadBehind: emptyAheadBehind,
  conflicts: [],
  loading: false,
  error: null,
  isGitRepo: null,   // null = unknown, true/false once checked
  remoteUrl: null,   // null = no remote configured
});

export const useGit = create((set, get) => ({
  activeProjectId: '',
  projects: {},
  commitMessages: {},
  expanded: {},

  setActiveProjectId: (id) => set({ activeProjectId: id }),

  setCommitMessage: (projectId, msg) =>
    set((s) => ({ commitMessages: { ...s.commitMessages, [projectId]: msg } })),

  toggleSection: (key) =>
    set((s) => ({ expanded: { ...s.expanded, [key]: !(s.expanded[key] ?? false) } })),

  collapseAllProjects: (projectIds) =>
    set((s) => {
      const updates = {};
      for (const id of projectIds) updates[`project-${id}`] = false;
      return { expanded: { ...s.expanded, ...updates } };
    }),

  getProject: (id) => get().projects[id] ?? emptyProjectState(),

  _patchProject: (id, patch) =>
    set((s) => ({
      projects: {
        ...s.projects,
        [id]: { ...emptyProjectState(), ...s.projects[id], ...patch },
      },
    })),

  async refreshAll(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;

    // Queue guard: if a refresh is already running, mark as needing another pass and return.
    // This prevents N concurrent git invocations from stacking when the user acts quickly.
    if (refreshLocks.get(id) === 'running') {
      refreshLocks.set(id, 'queued');
      return;
    }
    refreshLocks.set(id, 'running');

    // Only show the loading spinner on the very first load (no data yet).
    // Skipping the intermediate loading:true → loading:false cycle for background
    // refreshes halves the number of expensive re-renders on large change lists.
    const isFirstLoad = !get().projects[id];
    if (isFirstLoad) {
      get()._patchProject(id, { loading: true, error: null });
    }

    try {
      const isGitRepo = await invoke('git_is_repo', { projectId: id }).catch(() => false);

      if (!isGitRepo) {
        get()._patchProject(id, { isGitRepo: false, loading: false });
        return;
      }

      const limit = get().projects[id]?.statusLimit ?? STATUS_PAGE_SIZE;
      const [rawStatus, branches, aheadBehind, log, conflicts, remoteUrl] = await Promise.all([
        invoke('git_status', { projectId: id, limit }).catch(() => null),
        invoke('git_branches', { projectId: id }).catch(() => []),
        invoke('git_ahead_behind', { projectId: id }).catch(() => emptyAheadBehind),
        invoke('git_log', { projectId: id, maxCount: 30 }).catch(() => []),
        invoke('git_get_conflicts', { projectId: id }).catch(() => []),
        invoke('git_get_remote_url', { projectId: id }).catch(() => null),
      ]);
      const status = transformStatus(rawStatus);
      const currentBranch =
        (Array.isArray(branches) && branches.find((b) => b.is_head || b.is_current || b.current))
          ?.name ?? null;
      get()._patchProject(id, {
        status,
        statusCounts: countsFromRaw(rawStatus),
        statusTruncated: !!rawStatus?.truncated,
        branches,
        currentBranch,
        aheadBehind,
        log,
        conflicts,
        loading: false,
        isGitRepo: true,
        remoteUrl: remoteUrl ?? null,
      });
    } catch (err) {
      get()._patchProject(id, { loading: false, error: String(err) });
    } finally {
      const wasQueued = refreshLocks.get(id) === 'queued';
      refreshLocks.delete(id);
      // If another refresh was requested while this one was running, run one more pass.
      if (wasQueued) get().refreshAll(id);
    }
  },

  async refreshStatus(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    try {
      const limit = get().projects[id]?.statusLimit ?? STATUS_PAGE_SIZE;
      const [rawStatus, conflicts, aheadBehind] = await Promise.all([
        invoke('git_status', { projectId: id, limit }).catch(() => null),
        invoke('git_get_conflicts', { projectId: id }).catch(() => []),
        invoke('git_ahead_behind', { projectId: id }).catch(() => emptyAheadBehind),
      ]);
      get()._patchProject(id, {
        status: transformStatus(rawStatus),
        statusCounts: countsFromRaw(rawStatus),
        statusTruncated: !!rawStatus?.truncated,
        conflicts,
        aheadBehind,
      });
    } catch (err) {
      get()._patchProject(id, { error: String(err) });
    }
  },

  // Grow the per-project status window by one page, then re-fetch. Used by the
  // SCM panel's "Load more" rows on huge change lists.
  async loadMoreStatus(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    const cur = get().projects[id]?.statusLimit ?? STATUS_PAGE_SIZE;
    get()._patchProject(id, { statusLimit: cur + STATUS_PAGE_SIZE });
    await get().refreshStatus(id);
  },

  async stage(paths, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id || !paths?.length) return [];
    // Backend filters out paths matching .gitignore (a single ignored path
    // would otherwise abort the whole `git add` batch). Returns the skipped
    // paths so the caller can surface them.
    const skipped = (await invoke('git_stage', { projectId: id, paths })) ?? [];
    await get().refreshStatus(id);
    return skipped;
  },

  async unstage(paths, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id || !paths?.length) return;
    await invoke('git_unstage', { projectId: id, paths });
    await get().refreshStatus(id);
  },

  async discard(paths, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id || !paths?.length) return;
    await invoke('git_discard', { projectId: id, paths });
    await get().refreshStatus(id);
  },

  // Repo-wide bulk operations. These run a single git command over the whole
  // worktree, so they act on EVERY change — not just the rows currently loaded
  // into the windowed SCM panel (the path-list variants above would miss the
  // un-rendered remainder on a huge change list).
  async stageAll(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_stage_all', { projectId: id });
    await get().refreshStatus(id);
  },

  async unstageAll(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_unstage_all', { projectId: id });
    await get().refreshStatus(id);
  },

  async discardAll(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_discard_all', { projectId: id });
    await get().refreshStatus(id);
  },

  async commit(projectId) {
    const id = projectId ?? get().activeProjectId;
    const message = (get().commitMessages[id] ?? '').trim();
    if (!id || !message) return null;
    const hash = await invoke('git_commit', { projectId: id, message });
    set((s) => ({ commitMessages: { ...s.commitMessages, [id]: '' } }));
    await get().refreshAll(id);
    return hash;
  },

  async commitAndPush(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    const message = (get().commitMessages[id] ?? '').trim();
    if (!message) throw new Error('Commit message is empty');
    // Don't check stagedCount here — ensureStaged() in the caller handles it,
    // and checking state here creates a race condition. Let git commit fail
    // naturally if nothing is staged.
    const hash = await invoke('git_commit', { projectId: id, message });
    set((s) => ({ commitMessages: { ...s.commitMessages, [id]: '' } }));
    await invoke('git_push', { projectId: id });
    await get().refreshAll(id);
    return hash;
  },

  async sync(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_pull', { projectId: id });
    await invoke('git_push', { projectId: id });
    await get().refreshAll(id);
  },

  async checkoutBranch(branch, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id || !branch) return;
    await invoke('git_checkout_branch', { projectId: id, branch });
    await get().refreshAll(id);
    // Tell the file explorer to reload — branch checkout changes files on disk.
    window.dispatchEvent(new CustomEvent('rustic:branch-changed'));
  },

  async createBranch(branch, checkout = true, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id || !branch) return;
    await invoke('git_create_branch', { projectId: id, branch, checkout });
    await get().refreshAll(id);
  },

  async push(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_push', { projectId: id });
    // Push only changes the ahead/behind count — no need for the full 5-invoke refreshAll.
    const aheadBehind = await invoke('git_ahead_behind', { projectId: id }).catch(() => emptyAheadBehind);
    get()._patchProject(id, { aheadBehind });
  },

  async pull(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_pull', { projectId: id });
    await get().refreshAll(id);
  },

  async fetch(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_fetch', { projectId: id });
    // Fetch updates remote tracking — refresh branches + ahead/behind only.
    const [branches, aheadBehind] = await Promise.all([
      invoke('git_branches', { projectId: id }).catch(() => get().projects[id]?.branches ?? []),
      invoke('git_ahead_behind', { projectId: id }).catch(() => emptyAheadBehind),
    ]);
    const currentBranch =
      (Array.isArray(branches) && branches.find((b) => b.is_head || b.is_current || b.current))
        ?.name ?? get().projects[id]?.currentBranch ?? null;
    get()._patchProject(id, { branches, currentBranch, aheadBehind });
  },

  async resolveConflict(path, side, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_resolve_conflict', { projectId: id, path, side });
    await get().refreshStatus(id);
  },

  async undoLastCommit(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_undo_last_commit', { projectId: id });
    await get().refreshAll(id);
  },

  async loadCommitFiles(oid, projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id || !oid) return [];
    return invoke('git_commit_files', { projectId: id, oid }).catch(() => []);
  },

  async initRepo(projectId) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    await invoke('git_init', { projectId: id });
    await get().refreshAll(id);
  },

  async publishToGitHub(projectId, repoName, isPrivate) {
    const id = projectId ?? get().activeProjectId;
    if (!id) return;
    const cloneUrl = await invoke('github_create_repo', { name: repoName, private: isPrivate });
    await invoke('git_add_remote', { projectId: id, name: 'origin', url: cloneUrl });
    await invoke('git_publish_branch', { projectId: id });
    await get().refreshAll(id);
  },
}));
