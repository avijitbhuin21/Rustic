// "Open location" for finished transfers: the OS file manager on desktop;
// on the web build, bring the folder up in Rustic's own explorer.
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { IS_WEB } from '@/lib/platform';
import { useExplorer } from '@/state/explorer';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';

/** Normalised path for prefix comparison. */
function norm(p) {
  return String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
}

/** Reveal `path` (a folder) where the user can work with it. */
export async function openLocation(path) {
  if (!path) return;
  if (!IS_WEB) {
    try {
      await invoke('reveal_in_file_manager', { path });
    } catch (e) {
      toast.error(`Couldn't open ${path}: ${e?.message || e}`);
    }
    return;
  }
  const explorer = useExplorer.getState();
  const target = norm(path);
  let project = explorer.projects.find((p) => target === norm(p.root_path) || target.startsWith(`${norm(p.root_path)}/`));
  try {
    if (!project) project = await explorer.addProject(path);
  } catch (e) {
    toast.error(`Couldn't open ${path}: ${e?.message || e}`);
    return;
  }
  const layout = useLayout.getState();
  if (layout.activeSidebarPanel !== SIDEBAR_PANELS.EXPLORER || !layout.sidebarVisible) {
    layout.setActiveSidebarPanel(SIDEBAR_PANELS.EXPLORER);
  }
  if (project?.id) {
    explorer.setActiveProject(project.id);
    explorer.revealProjects('left', [project.id]);
    explorer.flashProject(project.id);
  }
}
