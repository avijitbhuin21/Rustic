import { listen } from '@tauri-apps/api/event';
import { useExplorer } from '@/state/explorer';
import { useGit } from '@/state/git';

// Trailing debounce per project so a burst of writes (agent edits, saves,
// checkouts) collapses into one git_status call — with a max-wait so a
// sustained stream of events (long agent task) still refreshes periodically
// instead of starving until the burst ends.
const DEBOUNCE_MS = 600;
const MAX_WAIT_MS = 3000;

const norm = (p) => (p ?? '').replace(/\\/g, '/').replace(/\/+$/, '');

const pending = new Map();

function scheduleRefresh(projectId, { full = false } = {}) {
  const now = Date.now();
  let entry = pending.get(projectId);
  if (!entry) {
    entry = { timer: null, firstQueuedAt: now, full: false };
    pending.set(projectId, entry);
  }
  entry.full = entry.full || full;
  if (entry.timer) clearTimeout(entry.timer);

  const elapsed = now - entry.firstQueuedAt;
  const wait = Math.max(0, Math.min(DEBOUNCE_MS, MAX_WAIT_MS - elapsed));
  entry.timer = setTimeout(() => {
    const wasFull = entry.full;
    pending.delete(projectId);
    const git = useGit.getState();
    if (wasFull) git.refreshAll(projectId);
    else git.refreshStatus(projectId);
  }, wait);
}

let wired = false;

/// Subscribes (once) to watcher fs-change events and keeps the git store's
/// status fresh for any project whose repo state was already loaded.
export function initGitAutoRefresh() {
  if (wired) return;
  wired = true;
  listen('rustic:fs-change', (e) => {
    const projectPath = norm(e.payload?.project_path);
    if (!projectPath) return;
    const project = useExplorer
      .getState()
      .projects.find((p) => norm(p.root_path) === projectPath);
    if (!project) return;
    // Only keep already-loaded repo state fresh. If the SCM panel never
    // fetched this project (or it isn't a git repo), there is nothing on
    // screen to go stale and no reason to shell out to git on every write.
    const gitState = useGit.getState().projects[project.id];
    if (!gitState || gitState.isGitRepo !== true) return;
    // git_changed = .git metadata moved (terminal `git add`/`commit`/checkout)
    // — branches, log and ahead/behind may all be stale, so do the full
    // refresh instead of the status-only one.
    scheduleRefresh(project.id, { full: e.payload?.git_changed === true });
  }).catch((err) => {
    console.error('git-auto-refresh: failed to subscribe to rustic:fs-change', err);
  });
}
