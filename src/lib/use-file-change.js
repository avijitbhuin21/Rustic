import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

// The watcher caps `changed_paths` at this size (see rustic-app watcher.rs).
// When the list is full it may be non-exhaustive, so we fall back to matching
// the file's parent directory against `changed_dirs`.
const CHANGED_PATHS_CAP = 512;

const norm = (p) => (p ?? '').replace(/\\/g, '/');

const parentDir = (p) => {
  const n = norm(p);
  const i = n.lastIndexOf('/');
  return i < 0 ? n : n.slice(0, i);
};

/// Returns true when a `rustic:fs-change` payload indicates `path` changed on disk.
export function fsChangeTouchesPath(payload, path) {
  const target = norm(path);
  if (!target) return false;
  const changedPaths = Array.isArray(payload?.changed_paths) ? payload.changed_paths : [];
  for (const p of changedPaths) {
    if (norm(p) === target) return true;
  }
  if (changedPaths.length >= CHANGED_PATHS_CAP) {
    const dir = parentDir(target);
    const changedDirs = Array.isArray(payload?.changed_dirs) ? payload.changed_dirs : [];
    for (const d of changedDirs) {
      if (norm(d) === dir) return true;
    }
  }
  return false;
}

/// Runs `onChange` whenever the watcher reports that `path` changed on disk.
export function useFileChangeEffect(path, onChange, { enabled = true } = {}) {
  const handlerRef = useRef(onChange);
  handlerRef.current = onChange;

  useEffect(() => {
    if (!enabled || !path) return undefined;
    let unlisten = null;
    let disposed = false;
    listen('rustic:fs-change', (e) => {
      if (fsChangeTouchesPath(e.payload ?? {}, path)) {
        handlerRef.current?.(e.payload);
      }
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    }).catch((err) => {
      console.error('useFileChangeEffect: failed to subscribe to rustic:fs-change', err);
    });
    return () => {
      disposed = true;
      if (typeof unlisten === 'function') unlisten();
    };
  }, [path, enabled]);
}

/// Returns a counter that increments each time `path` changes on disk — put it
/// in a load-effect's dependency array to re-fetch content live.
export function useFileReloadVersion(path, { enabled = true } = {}) {
  const [version, setVersion] = useState(0);
  useFileChangeEffect(path, () => setVersion((v) => v + 1), { enabled });
  return version;
}
