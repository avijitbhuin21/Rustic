import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Tree } from 'react-arborist';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { FileNode } from './file-node';
import { readDir, renameEntry } from '@/state/explorer';

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

export function FileTree({ rootPath, onOpenFile }) {
  const [data, setData] = useState([]);
  const [loading, setLoading] = useState(false);
  const [openIds, setOpenIds] = useState(() => new Set());
  const childrenCache = useRef(new Map());
  const treeRef = useRef(null);

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
      const next = entries.map(toNode);
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
      const payload = e.payload ?? {};
      const projectPath = norm(payload.project_path);
      if (projectPath !== rootNorm) return;
      const changed = Array.isArray(payload.changed_dirs) ? payload.changed_dirs : [];
      const cache = childrenCache.current;
      const loadedDirs = new Set(cache.keys());
      const toRefresh = new Set();
      for (const dir of changed) {
        const d = norm(dir);
        if (d === rootNorm) toRefresh.add(rootPath);
        else if (loadedDirs.has(dir)) toRefresh.add(dir);
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

  const onToggle = useCallback(async (id) => {
    setOpenIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
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
    }
  }, []);

  const handleActivate = useCallback((node) => {
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
          onRename={handleRename}
          onRefresh={refreshDir}
          disableDrag
          disableDrop
        >
          {FileNode}
        </Tree>
      )}
    </div>
  );
}
