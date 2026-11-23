import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { fetchFileBytes } from '@/lib/file-bytes';
import { useFileChangeEffect } from '@/lib/use-file-change';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';

/** Encodes bytes as base64 in chunks, so large decks do not overflow the call stack. */
function bytesToBase64(bytes) {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/** True when two byte arrays hold the same content. */
function sameBytes(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/** Tracks the app's light/dark mode so the editor chrome follows a theme switch. */
function useIsDark() {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains('dark'));
  useEffect(() => {
    const root = document.documentElement;
    const observer = new MutationObserver(() => setDark(root.classList.contains('dark')));
    observer.observe(root, { attributes: true, attributeFilter: ['class'] });
    return () => observer.disconnect();
  }, []);
  return dark;
}

/** PowerPoint (.pptx) editor by @pptx-editor.dev (Apache-2.0, lazy-loaded); layout in pptx-workspace.jsx. */
export default function PptxPreview({ tab }) {
  const [lib, setLib] = useState(null);
  const [doc, setDoc] = useState(null);
  const [error, setError] = useState(null);
  const [dirty, setDirty] = useState(false);
  const [diskChanged, setDiskChanged] = useState(false);
  const pptxRef = useRef(null);
  const refreshRef = useRef(null);
  const offChangeRef = useRef(null);
  const diskBytesRef = useRef(null);
  const dirtyRef = useRef(false);
  const savingRef = useRef(false);
  const syncRef = useRef({ running: false, pending: false, version: 0, retry: 0 });
  const dark = useIsDark();
  const tabSetDirty = useEditor((s) => s.setDirty);

  dirtyRef.current = dirty;

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setDoc(null);
    setDirty(false);
    setDiskChanged(false);
    (async () => {
      try {
        const [workspace, bytes] = await Promise.all([
          import('./pptx-workspace'),
          fetchFileBytes(tab.path),
        ]);
        if (cancelled) return;
        diskBytesRef.current = bytes;
        setLib({
          Workspace: workspace.PptxWorkspace,
          createDocumentRefresh: workspace.createDocumentRefresh,
          onEditorEvent: workspace.onEditorEvent,
        });
        setDoc(bytes);
      } catch (e) {
        if (!cancelled) setError(`Couldn't open presentation: ${e?.message || String(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, tab.id]);

  useEffect(() => () => offChangeRef.current?.(), []);

  const onReady = useCallback((editor) => {
    offChangeRef.current?.();
    refreshRef.current = lib.createDocumentRefresh(editor);
    // Loads and refreshes carry a `source`; only the user's own edits (and undo/redo) do not.
    offChangeRef.current = lib.onEditorEvent(editor, 'change', (_snapshot, change) => {
      if (!change?.source) setDirty(true);
      else if (change.source === 'load') setDirty(false);
    });
  }, [lib]);

  /** Shows `bytes` from disk in the open editor, keeping the current slide and zoom. */
  const applyDiskBytes = useCallback(async (bytes) => {
    const refresh = refreshRef.current;
    const handle = pptxRef.current;
    if (!refresh || !handle) return false;
    let result = null;
    try {
      const submission = await refresh.capture();
      try {
        result = await refresh.applyUpdate({ submission, sequence: 1, bytes });
      } finally {
        refresh.finish(submission);
      }
    } catch {
      result = null;
    }
    if (result && !result.ok && (result.code === 'invalid-document' || result.code === 'invalid-result')) {
      return false;
    }
    if (!result?.ok) {
      const loaded = await handle.load(bytes, { startSlide: handle.snapshot().currentSlide, quiet: true });
      if (!loaded?.ok) return false;
    } else {
      refresh.highlightChanges();
    }
    diskBytesRef.current = bytes;
    setDirty(false);
    setDiskChanged(false);
    return true;
  }, []);

  /** Reads the file again and either shows it or, with unsaved edits, raises the notice. */
  const syncFromDisk = useCallback(async ({ force = false } = {}) => {
    const s = syncRef.current;
    if (s.running) { s.pending = true; return; }
    s.running = true;
    try {
      do {
        s.pending = false;
        s.version += 1;
        const bytes = await fetchFileBytes(tab.path, { version: `${Date.now()}-${s.version}` });
        if (sameBytes(bytes, diskBytesRef.current)) continue;
        if (dirtyRef.current && !force) { setDiskChanged(true); continue; }
        const applied = await applyDiskBytes(bytes);
        // A half-written file does not open; the agent may still be writing it.
        if (!applied && s.retry < 3) {
          s.retry += 1;
          setTimeout(() => syncFromDisk({ force }), 600);
        } else {
          s.retry = 0;
        }
      } while (s.pending);
    } catch (e) {
      console.error('pptx: could not re-read the file', e);
    } finally {
      s.running = false;
    }
  }, [tab.path, applyDiskBytes]);

  useFileChangeEffect(tab.path, () => {
    if (!savingRef.current) syncFromDisk();
  }, { enabled: !!doc });

  const onSave = async () => {
    const handle = pptxRef.current;
    if (!handle || savingRef.current) return;
    savingRef.current = true;
    try {
      const out = handle.save();
      if (!out) throw new Error('the presentation could not be serialised');
      await invoke('write_file_base64', { path: tab.path, data: bytesToBase64(out) });
      diskBytesRef.current = out;
      setDirty(false);
      setDiskChanged(false);
      toast.success('Saved');
    } catch (e) {
      toast.error(`Save failed: ${typeof e === 'string' ? e : e?.message || String(e)}`);
    } finally {
      savingRef.current = false;
    }
  };

  useEffect(() => {
    setActiveSaver(onSave);
    return () => clearActiveSaver(onSave);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.path, dirty]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (!lib || !doc) {
    return (
      <div className="flex h-full w-full flex-col gap-3 p-6">
        <Skeleton className="h-9 w-full" />
        <Skeleton className="min-h-0 w-full flex-1" />
        <Skeleton className="h-24 w-full" />
      </div>
    );
  }

  const Editor = lib.Workspace;
  return (
    <div className="rustic-pptx-host flex h-full w-full flex-col">
      {diskChanged && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/40 px-3 py-1.5 text-xs text-muted-foreground">
          <span className="min-w-0 truncate">
            This file changed on disk while you have unsaved edits. Saving overwrites the version on disk.
          </span>
          <Button
            variant="outline"
            size="sm"
            className="ml-auto h-6 shrink-0 px-2 text-xs"
            onClick={() => syncFromDisk({ force: true })}
          >
            Discard my edits and reload
          </Button>
        </div>
      )}
      <div className="min-h-0 flex-1">
        <Editor
          ref={pptxRef}
          bytes={doc}
          colorMode={dark ? 'dark' : 'light'}
          onReady={onReady}
        />
      </div>
    </div>
  );
}
