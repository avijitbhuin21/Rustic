import { useEffect, useRef } from 'react';
import { useEditor } from '@/state/editor';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { usePanelSide } from '@/lib/panel-side';

/// True when `path` is `root` or lives beneath it (separator + case normalized like the OS).
function pathWithin(path, root) {
  const norm = (p) => String(p || '').replace(/[\\/]+/g, '/').replace(/\/+$/, '');
  let a = norm(path);
  let b = norm(root);
  if (!a || !b) return false;
  if (navigator.platform?.startsWith('Win')) {
    a = a.toLowerCase();
    b = b.toLowerCase();
  }
  return a === b || a.startsWith(`${b}/`);
}

/// Returns the projects the user is "in" right now: the active editor file's project first, then the active chat's project. `ids` is de-duplicated and ordered by priority.
export function resolveContextProjects() {
  const projects = useExplorer.getState().projects || [];
  const ed = useEditor.getState();
  const group = (ed.groups || []).find((g) => g.id === ed.activeGroupId) || ed.groups?.[0];
  const tab = group?.tabs?.find((t) => t.id === group.activeId);
  const filePath = tab?.path || null;
  const fileProject = filePath
    ? projects.find((p) => p.root_path && pathWithin(filePath, p.root_path)) || null
    : null;
  const chatId = useAgent.getState().activeProject?.id;
  const chatProject = chatId ? projects.find((p) => p.id === chatId) || null : null;
  const ids = [];
  for (const p of [fileProject, chatProject]) {
    if (p && !ids.includes(p.id)) ids.push(p.id);
  }
  return { primary: ids[0] || null, ids, filePath };
}

/// Runs `apply(ctx, side)` each time the sidebar panel `panelId` (on this side) becomes visible and there is at least one context project — used to auto-expand / select the current project in Explorer, Search and Source Control.
export function useContextProjectReveal(panelId, apply) {
  const side = usePanelSide();
  const visible = useLayout((s) =>
    side === 'right'
      ? s.rightPanel === panelId
      : s.activeSidebarPanel === panelId && s.sidebarVisible,
  );
  const projectsLoaded = useExplorer((s) => s.hasLoaded);
  const applyRef = useRef(apply);
  applyRef.current = apply;
  useEffect(() => {
    if (!visible || !projectsLoaded) return;
    const ctx = resolveContextProjects();
    if (ctx.ids.length === 0) return;
    applyRef.current(ctx, side);
  }, [visible, projectsLoaded, side]);
}
