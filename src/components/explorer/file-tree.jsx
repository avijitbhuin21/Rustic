import React, { useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, forwardRef } from 'react';
import { Tree } from 'react-arborist';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { FileNode } from './file-node';
import { contextMenuState } from './context-menu-state';
import { createFile, createFolder, readDir, renameEntry, moveEntry, deleteEntry, useExplorer } from '@/state/explorer';
import { confirm } from '@/components/confirm-dialog';

const ROW_HEIGHT = 24;
const SKELETON_WIDTHS = [62, 45, 78, 53];
const SKELETON_HEIGHT = SKELETON_WIDTHS.length * ROW_HEIGHT;

function SkeletonRows() {
  return (
    <div className="w-full">
      {SKELETON_WIDTHS.map((w, i) => (
        <div key={i} className="flex h-6 items-center gap-1 px-1">
          <div className="size-3 rounded-sm bg-muted/50 animate-pulse" />
          <div className="h-2.5 rounded bg-muted/50 animate-pulse" style={{ width: `${w}%` }} />
        </div>
      ))}
    </div>
  );
}

function toNode(entry) {
  return {
    id: entry.path,
    name: entry.name,
    is_dir: entry.is_dir,
    path: entry.path,
    children: entry.is_dir ? null : undefined,
  };
}

function injectChildren(nodes, parentId, children) {
  return nodes.map((n) => {
    if (n.id === parentId) return { ...n, children };
    if (n.children) return { ...n, children: injectChildren(n.children, parentId, children) };
    return n;
  });
}

// Reattach previously-loaded children to fresh entries coming out of readDir.
// Fresh entries from `toNode` always have `children: null` for directories —
// without this rehydration, a refresh of a parent (especially the root) wipes
// every nested expanded folder back to "not loaded" in `data`, while the cache
// still holds their real children. That mismatch is what makes folders stop
// opening after an agent edit: react-arborist sees `children: null`, treats
// the node as a leaf, and the `cache.has(id)` short-circuit in `onToggle`
// blocks re-fetch, so `data` never recovers without a manual refresh.
function rehydrateFromCache(nodes, cache) {
  return nodes.map((n) => {
    if (!n.is_dir) return n;
    const cached = cache.get(n.id);
    if (!cached) return n;
    return { ...n, children: rehydrateFromCache(cached, cache) };
  });
}

function countVisible(nodes, openIds) {
  let n = 0;
  for (const node of nodes) {
    n += 1;
    if (node.is_dir && openIds.has(node.id) && Array.isArray(node.children)) {
      n += countVisible(node.children, openIds);
    }
  }
  return n;
}

async function findUniqueName(parentDir, base) {
  const entries = await readDir(parentDir);
  const existing = new Set(entries.map((e) => e.name));
  if (!existing.has(base)) return base;
  let i = 2;
  while (existing.has(`${base} ${i}`)) i++;
  return `${base} ${i}`;
}

export const FileTree = forwardRef(function FileTree({ rootPath, onOpenFile }, ref) {
  const [data, setData] = useState([]);
  const [loading, setLoading] = useState(false);
  const [openIds, setOpenIds] = useState(() => new Set());
  const childrenCache = useRef(new Map());
  const treeRef = useRef(null);
  const lastNodeRef = useRef(null);

  useEffect(() => {
    let cancelled = false;
    childrenCache.current.clear();
    setOpenIds(new Set());
    setData([]);
    if (!rootPath) return;
    setLoading(true);
    readDir(rootPath).then((entries) => {
      if (cancelled) return;
      setData(entries.map(toNode));
      setLoading(false);
    }).catch((e) => {
      if (cancelled) return;
      console.error('FileTree: readDir(root) failed', rootPath, e);
      toast.error(`Failed to read project: ${e?.message ?? e}`);
      setLoading(false);
    });
    return () => { cancelled = true; };
  }, [rootPath]);

  const refreshDir = useCallback(async (dirPath) => {
    const target = dirPath ?? rootPath;
    if (!target) return;
    try {
      const entries = await readDir(target);
      // Reattach cached children to any subdir that the user previously
      // expanded — otherwise `setData(next)` (root refresh) or
      // `injectChildren` (parent refresh) wipes nested loaded folders back
      // to `children: null`, which collides with the cache.has(id)
      // short-circuit in onToggle and leaves them un-openable. See the
      // comment on rehydrateFromCache.
      const next = rehydrateFromCache(entries.map(toNode), childrenCache.current);
      if (target === rootPath) {
        setData(next);
      } else {
        childrenCache.current.set(target, next);
        setData((prev) => injectChildren(prev, target, next));
      }
    } catch (e) {
      console.error('FileTree: refreshDir failed', target, e);
    }
  }, [rootPath]);

  // Create an empty file/folder with an auto-picked placeholder name, then
  // drop straight into rename mode so the user types the real name into the
  // tree (instead of through a window.prompt). Used by both the project
  // header's new-file/folder buttons (parentDir = project root) and the
  // file-node context menu (parentDir = folder being right-clicked).
  const createAndEdit = useCallback(async (parentDir, kind) => {
    try {
      const base = kind === 'folder' ? 'new-folder' : 'new-file';
      const name = await findUniqueName(parentDir, base);
      const newPath = kind === 'folder'
        ? await createFolder(parentDir, name)
        : await createFile(parentDir, name);

      // Make sure the new node lands in our `data` state — refreshDir handles
      // both the root case (replaces data) and the nested case (injectChildren
      // into the parent), and the rehydrateFromCache pass inside it preserves
      // any sibling folders the user had already expanded.
      await refreshDir(parentDir);

      // Open the parent so the new node is actually visible before we ask
      // react-arborist to put it in edit mode. The root is always "open" from
      // react-arborist's perspective, so this only matters for subfolders.
      if (parentDir !== rootPath) {
        setOpenIds((prev) => {
          if (prev.has(parentDir)) return prev;
          const next = new Set(prev);
          next.add(parentDir);
          return next;
        });
        await new Promise((r) => requestAnimationFrame(r));
        try { treeRef.current?.open?.(parentDir); } catch {}
      }

      // Wait two frames: setData/setOpenIds need to commit + react-arborist
      // needs a frame to settle its internal open state before edit() can
      // find the new node. One frame is sometimes enough but two is reliable.
      await new Promise((r) => requestAnimationFrame(r));
      await new Promise((r) => requestAnimationFrame(r));
      try {
        treeRef.current?.edit?.(newPath);
      } catch (e) {
        console.error('createAndEdit: tree.edit failed', e);
      }
    } catch (e) {
      toast.error(String(e));
    }
  }, [rootPath, refreshDir]);

  // Drag-to-move: a node was dropped onto a folder. Move it on disk, then
  // refresh the source's parent and the destination so both reflect reality.
  const handleMoveEntry = useCallback(async (srcPath, destDir) => {
    if (!srcPath || !destDir) return;
    const norm = (p) => p.replace(/\\/g, '/').replace(/\/+$/, '');
    const s = norm(srcPath);
    const d = norm(destDir);
    const srcParentOrig = srcPath.replace(/[\\/][^\\/]+$/, '');
    if (d === s) return;
    if (d === norm(srcParentOrig)) return; // already in this folder — no-op
    if (d.startsWith(s + '/')) {
      toast.error("Can't move a folder into itself");
      return;
    }
    try {
      await moveEntry(srcPath, destDir);
      toast.success('Moved');
      await refreshDir(srcParentOrig);
      await refreshDir(destDir);
    } catch (e) {
      toast.error(String(e));
    }
  }, [refreshDir]);

  // Expose `moveInto` so the project section's root drop zones can move a
  // dragged node to the project root (the tree rows can only drop into nested
  // folders — there's no folder row representing the root).
  const moveInto = useCallback(
    (srcPath) => handleMoveEntry(srcPath, rootPath),
    [handleMoveEntry, rootPath],
  );

  // The F2/Delete window events are broadcast to EVERY mounted FileTree, and
  // each tree keeps its own (potentially stale) lastNodeRef from an earlier
  // click. Only the tree whose remembered node matches the GLOBAL last
  // selection may act — otherwise pressing Delete after switching projects
  // would also delete a file in the previously-clicked tree.
  const ownsLastSelection = useCallback(() => {
    const node = lastNodeRef.current;
    if (!node) return false;
    const last = useExplorer.getState().lastSelectedNode;
    return !!last && last.path === node.data.path;
  }, []);

  const renameSelected = useCallback(() => {
    if (!ownsLastSelection()) return;
    const node = lastNodeRef.current;
    try {
      treeRef.current?.edit?.(node.id);
    } catch (e) {
      console.error('renameSelected: tree.edit failed', e);
    }
  }, [ownsLastSelection]);

  const deleteSelected = useCallback(async () => {
    // Multi-selection (Ctrl/Shift-click) owned by THIS tree: delete the set.
    const sel = useExplorer.getState().selection;
    if (sel.rootPath === rootPath && sel.items.length > 1) {
      const norm = (p) => p.replace(/\\/g, '/');
      // Drop items nested inside another selected folder — deleting the
      // folder already removes them, and a second delete would just error.
      const folderPaths = sel.items.filter((it) => it.isDir).map((it) => norm(it.path));
      const items = sel.items.filter((it) => {
        const p = norm(it.path);
        return !folderPaths.some((f) => f !== p && p.startsWith(f + '/'));
      });
      const ok = await confirm({
        title: `Delete ${items.length} items?`,
        description: 'This cannot be undone.',
        confirmLabel: 'Delete',
        destructive: true,
      });
      if (!ok) return;
      const parents = new Set();
      let failed = 0;
      for (const it of items) {
        try {
          await deleteEntry(it.path);
          parents.add(it.path.replace(/[\\/][^\\/]+$/, ''));
        } catch (e) {
          failed += 1;
          console.error('FileTree: delete failed', it.path, e);
        }
      }
      for (const p of parents) await refreshDir(p);
      if (failed) toast.error(`Deleted ${items.length - failed} item${items.length - failed === 1 ? '' : 's'}, ${failed} failed`);
      else toast.success(`Deleted ${items.length} items`);
      useExplorer.getState().clearSelection();
      try { treeRef.current?.deselectAll?.(); } catch {}
      return;
    }

    if (!ownsLastSelection()) return;
    const node = lastNodeRef.current;
    const ok = await confirm({
      title: `Delete ${node.data.name}?`,
      description: 'This cannot be undone.',
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok) return;
    const parentDir = node.data.path.replace(/[\\/][^\\/]+$/, '');
    try {
      await deleteEntry(node.data.path);
      toast.success(`Deleted ${node.data.name}`);
      await refreshDir(parentDir);
    } catch (e) {
      toast.error(String(e));
    }
  }, [refreshDir, rootPath, ownsLastSelection]);

  useImperativeHandle(
    ref,
    () => ({ createAndEdit, moveInto, renameSelected, deleteSelected }),
    [createAndEdit, moveInto, renameSelected, deleteSelected],
  );

  // Reload the entire tree when a branch checkout happens.
  useEffect(() => {
    if (!rootPath) return;
    const handleBranchChange = () => {
      childrenCache.current.clear();
      setOpenIds(new Set());
      setData([]);
      setLoading(true);
      readDir(rootPath)
        .then((entries) => { setData(entries.map(toNode)); setLoading(false); })
        .catch((e) => {
          console.error('FileTree: readDir on branch change failed', rootPath, e);
          toast.error(`Failed to reload project: ${e?.message ?? e}`);
          setLoading(false);
        });
    };
    window.addEventListener('rustic:branch-changed', handleBranchChange);
    return () => window.removeEventListener('rustic:branch-changed', handleBranchChange);
  }, [rootPath]);

  useEffect(() => {
    if (!rootPath) return;
    let unlisten = null;
    const norm = (p) => (p ?? '').replace(/\\/g, '/');
    const rootNorm = norm(rootPath);
    listen('rustic:fs-change', (e) => {
      // While an inline rename is in progress, refreshing would replace `data`
      // and remount the edited row — which blurs the rename <input> and fires
      // its onBlur → node.reset(), cancelling the rename before the user can
      // type. The OS file-watcher fires far more aggressively on the desktop
      // build, which is exactly why rename "exited by itself" there. Skip the
      // refresh during the edit; handleRename re-syncs the tree on submit and
      // the next fs event catches anything missed.
      if (treeRef.current?.editingId != null) return;
      const payload = e.payload ?? {};
      const projectPath = norm(payload.project_path);
      if (projectPath !== rootNorm) return;
      const changed = Array.isArray(payload.changed_dirs) ? payload.changed_dirs : [];

      // Build a normalised→original lookup off the cache. The watcher
      // emits `changed_dirs` with forward slashes (it does the conversion
      // on the Rust side), but the cache keys are whatever `readDir`
      // returned — OS-native paths, which on Windows use backslashes.
      // Without normalising both sides, every nested-dir change misses
      // the cache and the tree silently goes stale. Earlier versions
      // compared `loadedDirs.has(dir)` directly which is why
      // delete/restore from inside a folder never refreshed unless the
      // user did a full Ctrl+R.
      const cache = childrenCache.current;
      const loadedNormToOrig = new Map();
      for (const key of cache.keys()) {
        loadedNormToOrig.set(norm(key), key);
      }

      const toRefresh = new Set();
      for (const dir of changed) {
        const d = norm(dir);
        if (d === rootNorm) {
          toRefresh.add(rootPath);
          continue;
        }
        const orig = loadedNormToOrig.get(d);
        if (orig !== undefined) toRefresh.add(orig);
      }
      for (const d of toRefresh) refreshDir(d);
    }).then((un) => {
      unlisten = un;
    }).catch((err) => {
      console.error('FileTree: failed to subscribe to rustic:fs-change', err);
    });
    return () => {
      if (typeof unlisten === 'function') unlisten();
    };
  }, [rootPath, refreshDir]);

  // Manual full-tree refresh — wired to the Explorer header's refresh
  // button via the global `rustic:tree-refresh` window event. Useful when
  // the OS watcher misses changes (network drives, WSL paths, sleep/wake
  // races) so the user has an escape hatch beyond Ctrl+R.
  useEffect(() => {
    if (!rootPath) return;
    const onForceRefresh = () => {
      // Drop the entire children cache so every previously-expanded
      // subdir re-fetches lazily on next toggle. Also reset openIds —
      // a full reload starts react-arborist's internal open state from
      // the `openByDefault={false}` baseline, so keeping stale openIds
      // would put us back in the drift state described in onToggle.
      childrenCache.current.clear();
      setOpenIds(new Set());
      readDir(rootPath)
        .then((entries) => setData(entries.map(toNode)))
        .catch((e) => {
          console.error('FileTree: manual refresh failed', rootPath, e);
        });
    };
    
    const onRenameRequest = () => {
      renameSelected();
    };
    
    const onDeleteRequest = () => {
      deleteSelected();
    };
    
    window.addEventListener('rustic:tree-refresh', onForceRefresh);
    window.addEventListener('rustic:explorer-rename', onRenameRequest);
    window.addEventListener('rustic:explorer-delete', onDeleteRequest);
    return () => {
      window.removeEventListener('rustic:tree-refresh', onForceRefresh);
      window.removeEventListener('rustic:explorer-rename', onRenameRequest);
      window.removeEventListener('rustic:explorer-delete', onDeleteRequest);
    };
  }, [rootPath, renameSelected, deleteSelected]);

  const onToggle = useCallback(async (id) => {
    // Trust react-arborist's *post-toggle* state rather than XORing our
    // own openIds. Any time refreshDir replaces a node's `children`
    // array, react-arborist can reset that node's internal open flag —
    // which means openIds drifts from the truth. Once they diverge, the
    // XOR logic flips both fields in opposite directions on every click
    // and the user gets stuck unable to expand the folder. Asking the
    // Tree for the authoritative state removes the drift entirely.
    const api = treeRef.current;
    const isOpenAfter = typeof api?.isOpen === 'function'
      ? api.isOpen(id)
      : !openIds.has(id); // sensible fallback before the ref attaches
    setOpenIds((prev) => {
      const has = prev.has(id);
      if (isOpenAfter === has) return prev;
      const next = new Set(prev);
      if (isOpenAfter) next.add(id);
      else next.delete(id);
      return next;
    });
    if (!isOpenAfter) return; // closing — nothing to fetch
    const cache = childrenCache.current;
    if (cache.has(id)) return;
    try {
      const entries = await readDir(id);
      cache.set(id, entries.map(toNode));
      setData((prev) => injectChildren(prev, id, cache.get(id)));
    } catch (e) {
      // Don't cache `[]` on failure: that would lock the folder shut until
      // a full reload, since cache.has(id) would short-circuit future clicks.
      // Roll back openIds so the chevron returns to closed and the next click
      // retries readDir.
      console.error('FileTree: readDir failed for', id, e);
      toast.error(`Failed to open folder: ${e?.message ?? e}`);
      setOpenIds((prev) => {
        if (!prev.has(id)) return prev;
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
      try { api?.close?.(id); } catch {}
    }
  }, [openIds]);

  const handleActivate = useCallback((node) => {
    if (contextMenuState.active) return;
    lastNodeRef.current = node;
    if (!node || node.data?.is_dir) return;
    onOpenFile?.(node.data.path);
  }, [onOpenFile]);

  const handleRename = useCallback(async ({ id, name, node }) => {
    if (!name || name === node.data.name) return;
    const oldPath = node.data.path;
    const parentDir = oldPath.replace(/[\\/][^\\/]+$/, '');
    try {
      await renameEntry(oldPath, name);
      toast.success(`Renamed to ${name}`);
      await refreshDir(parentDir);
    } catch (e) {
      toast.error(String(e));
    }
  }, [refreshDir]);

  const handleNodeClick = useCallback((node) => {
    lastNodeRef.current = node;
  }, []);

  // Mirror react-arborist's selection into the explorer store so keyboard
  // copy/cut/delete and the context menu can act on the whole multi-set.
  const handleSelect = useCallback((nodes) => {
    const items = (nodes ?? []).map((n) => ({
      path: n.data.path,
      isDir: !!n.data.is_dir,
      name: n.data.name,
    }));
    useExplorer.getState().setSelection(rootPath, items);
  }, [rootPath]);

  // When another project's tree takes the selection, drop this tree's
  // highlight — two trees showing "selected" rows at once would make the
  // multi-ops ambiguous to the user.
  const selectionRoot = useExplorer((s) => s.selection.rootPath);
  useEffect(() => {
    if (selectionRoot !== null && selectionRoot !== rootPath) {
      try { treeRef.current?.deselectAll?.(); } catch {}
    }
  }, [selectionRoot, rootPath]);

  const visibleCount = useMemo(() => countVisible(data, openIds), [data, openIds]);
  const treeHeight = loading ? SKELETON_HEIGHT : Math.max(ROW_HEIGHT, visibleCount * ROW_HEIGHT);

  if (!loading && data.length === 0) return null;

  return (
    <div
      className="w-full overflow-hidden"
      style={{ height: treeHeight, transition: 'height 200ms ease' }}
    >
      {loading ? (
        <SkeletonRows />
      ) : (
        <Tree
          ref={treeRef}
          data={data}
          openByDefault={false}
          width="100%"
          height={treeHeight}
          indent={12}
          rowHeight={ROW_HEIGHT}
          onToggle={onToggle}
          onActivate={handleActivate}
          onSelect={handleSelect}
          onRename={handleRename}
          onRefresh={refreshDir}
          onCreateAndEdit={createAndEdit}
          onMoveEntry={handleMoveEntry}
          onNodeClick={handleNodeClick}
          disableDrag
          disableDrop
        >
          {FileNode}
        </Tree>
      )}
    </div>
  );
});
