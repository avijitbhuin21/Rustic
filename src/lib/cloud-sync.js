import { invoke } from '@tauri-apps/api/core';
import { IS_WEB } from '@/lib/platform';

const URL_KEY = 'rustic.remoteBackend.url';

/// The deployed rustic-server URL saved by the Remote Backend settings.
export function cloudUrl() {
  try {
    return localStorage.getItem(URL_KEY) || '';
  } catch {
    return '';
  }
}

/// True when per-project cloud sync can run: desktop build, a saved server URL
/// and a password remembered from a verified connection.
export async function cloudSyncReady() {
  if (IS_WEB || !cloudUrl()) return false;
  try {
    return !!(await invoke('cloud_sync_has_credentials'));
  } catch {
    return false;
  }
}

/// Push or pull a single project's files. Resolves to the backend's summary.
export async function syncProject(direction, projectId) {
  const url = cloudUrl();
  if (!url) throw new Error('No cloud server configured — add one in Settings › Remote Backend');
  const command = direction === 'push' ? 'cloud_sync_push_project' : 'cloud_sync_pull_project';
  return invoke(command, { url, projectId });
}
