import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';

export const gitStore = createStore({
  // Map of projectId -> { branch, files: [{ path, status, is_staged }] }
  projectStatuses: {},
  // Map of projectId -> { ahead, behind }
  projectSyncStatus: {},
  // Map of projectId -> [ConflictFile]
  projectConflicts: {},
  // Map of projectId -> [CommitInfo]
  projectCommits: {},
  isLoading: false,
  hasToken: false,
});

export async function refreshGitStatus(projectId) {
  try {
    const status = await api.gitStatus(projectId);
    if (!status) return;

    const statuses = { ...gitStore.getState('projectStatuses') };
    statuses[projectId] = status;
    gitStore.setState({ projectStatuses: statuses });

    // Also refresh ahead/behind and commit log
    refreshAheadBehind(projectId);
    refreshCommitLog(projectId);
  } catch (e) {
    // Not a git repo or error — clear status
    const statuses = { ...gitStore.getState('projectStatuses') };
    statuses[projectId] = null;
    gitStore.setState({ projectStatuses: statuses });
  }
}

export async function refreshAllGitStatuses(projects) {
  for (const p of projects) {
    await refreshGitStatus(p.id);
  }
}

export async function refreshAheadBehind(projectId) {
  try {
    const result = await api.gitAheadBehind(projectId);
    if (!result) return;
    const sync = { ...gitStore.getState('projectSyncStatus') };
    sync[projectId] = result;
    gitStore.setState({ projectSyncStatus: sync });
  } catch {
    // Ignore — no remote or detached HEAD
  }
}

export async function stageFiles(projectId, paths) {
  try {
    await api.gitStage(projectId, paths);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to stage:', e);
  }
}

export async function unstageFiles(projectId, paths) {
  try {
    await api.gitUnstage(projectId, paths);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to unstage:', e);
  }
}

export async function commitChanges(projectId, message) {
  try {
    // Auto-stage all unstaged changes if nothing is staged (like VS Code)
    const status = gitStore.getState('projectStatuses')[projectId];
    if (status) {
      const staged = status.files.filter(f => f.is_staged);
      const unstaged = status.files.filter(f => !f.is_staged);
      if (staged.length === 0 && unstaged.length > 0) {
        await api.gitStage(projectId, unstaged.map(f => f.path));
      }
    }
    await api.gitCommit(projectId, message);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to commit:', e);
    throw e;
  }
}

export async function commitAndPush(projectId, message) {
  await commitChanges(projectId, message);
  await pushChanges(projectId);
}

export async function addToGitignore(projectId, pattern) {
  try {
    await api.gitAddToGitignore(projectId, pattern);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to add to .gitignore:', e);
  }
}

export async function discardChanges(projectId, paths) {
  try {
    await api.gitDiscard(projectId, paths);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to discard:', e);
  }
}

export async function pushChanges(projectId) {
  try {
    gitStore.setState({ isLoading: true });
    await api.gitPush(projectId);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to push:', e);
    throw e;
  } finally {
    gitStore.setState({ isLoading: false });
  }
}

export async function pullChanges(projectId) {
  try {
    gitStore.setState({ isLoading: true });
    await api.gitPull(projectId);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to pull:', e);
    throw e;
  } finally {
    gitStore.setState({ isLoading: false });
  }
}

export async function fetchChanges(projectId) {
  try {
    gitStore.setState({ isLoading: true });
    await api.gitFetch(projectId);
    await refreshAheadBehind(projectId);
  } catch (e) {
    console.error('Failed to fetch:', e);
    throw e;
  } finally {
    gitStore.setState({ isLoading: false });
  }
}

export async function initRepo(projectId) {
  try {
    await api.gitInit(projectId);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to init:', e);
  }
}

export async function checkoutBranch(projectId, branch) {
  try {
    await api.gitCheckoutBranch(projectId, branch);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to checkout:', e);
    throw e;
  }
}

export async function createBranch(projectId, branch, checkout = true) {
  try {
    await api.gitCreateBranch(projectId, branch, checkout);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to create branch:', e);
    throw e;
  }
}

export async function rebase(projectId, ontoBranch) {
  try {
    gitStore.setState({ isLoading: true });
    await api.gitRebase(projectId, ontoBranch);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Rebase failed:', e);
    await refreshConflicts(projectId);
    throw e;
  } finally {
    gitStore.setState({ isLoading: false });
  }
}

export async function rebaseContinue(projectId) {
  try {
    await api.gitRebaseContinue(projectId);
    await refreshGitStatus(projectId);
    // Clear conflicts on success
    const conflicts = { ...gitStore.getState('projectConflicts') };
    delete conflicts[projectId];
    gitStore.setState({ projectConflicts: conflicts });
  } catch (e) {
    console.error('Rebase continue failed:', e);
    throw e;
  }
}

export async function rebaseAbort(projectId) {
  try {
    await api.gitRebaseAbort(projectId);
    await refreshGitStatus(projectId);
    const conflicts = { ...gitStore.getState('projectConflicts') };
    delete conflicts[projectId];
    gitStore.setState({ projectConflicts: conflicts });
  } catch (e) {
    console.error('Rebase abort failed:', e);
  }
}

export async function refreshConflicts(projectId) {
  try {
    const result = await api.gitGetConflicts(projectId);
    const conflicts = { ...gitStore.getState('projectConflicts') };
    conflicts[projectId] = result || [];
    gitStore.setState({ projectConflicts: conflicts });
  } catch {
    // No conflicts
  }
}

export async function resolveConflict(projectId, path, side) {
  try {
    await api.gitResolveConflict(projectId, path, side);
    await refreshConflicts(projectId);
    await refreshGitStatus(projectId);
  } catch (e) {
    console.error('Failed to resolve conflict:', e);
    throw e;
  }
}

export async function mergeCommit(projectId) {
  try {
    await api.gitMergeCommit(projectId);
    await refreshGitStatus(projectId);
    const conflicts = { ...gitStore.getState('projectConflicts') };
    delete conflicts[projectId];
    gitStore.setState({ projectConflicts: conflicts });
  } catch (e) {
    console.error('Merge commit failed:', e);
    throw e;
  }
}

export async function setGitToken(token) {
  try {
    await api.gitSetToken(token);
    gitStore.setState({ hasToken: !!token });
  } catch (e) {
    console.error('Failed to set token:', e);
  }
}

export async function refreshCommitLog(projectId, maxCount = 50) {
  try {
    const commits = await api.gitLog(projectId, maxCount);
    const all = { ...gitStore.getState('projectCommits') };
    all[projectId] = commits || [];
    gitStore.setState({ projectCommits: all });
  } catch {
    // No commits or not a git repo
    const all = { ...gitStore.getState('projectCommits') };
    all[projectId] = [];
    gitStore.setState({ projectCommits: all });
  }
}

export async function getCommitFiles(projectId, oid) {
  try {
    return await api.gitCommitFiles(projectId, oid);
  } catch (e) {
    console.error('Failed to get commit files:', e);
    return [];
  }
}

export async function checkGitToken() {
  try {
    const has = await api.gitGetToken();
    gitStore.setState({ hasToken: !!has });
  } catch {
    gitStore.setState({ hasToken: false });
  }
}
