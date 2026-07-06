import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';
import './docx-preview.css';

// We dynamic-import the DOCX editor + its CSS so the ~500 KB bundle
// (ProseMirror + the OOXML parser/serializer) only loads when the user
// actually opens a .docx file, not when EditorPane mounts.

function base64ToBytes(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
function bytesToBase64(bytes) {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export default function DocxPreview({ tab }) {
  // The DocxEditor itself is lazy-loaded. We keep a ref to it so we can
  // call `.save()` on Ctrl+S.
  const [EditorComponent, setEditorComponent] = useState(null);
  const [buffer, setBuffer] = useState(null);
  const [error, setError] = useState(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const editorRef = useRef(null);
  // Used to ignore the change event burst that fires during initial
  // document load — without this gate, opening a file would instantly
  // mark it dirty.
  const readyForDirtyRef = useRef(false);
  const tabSetDirty = useEditor((s) => s.setDirty);

  const reloadVersion = useFileReloadVersion(tab.path, { enabled: !dirty });

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setBuffer(null);
    setDirty(false);
    readyForDirtyRef.current = false;

    (async () => {
      try {
        // Pull the editor + the file bytes in parallel; the editor
        // bundle is the slow part on first open.
        const [editorMod, _css, res] = await Promise.all([
          import('@eigenpal/docx-editor-react'),
          import('@eigenpal/docx-editor-react/styles.css'),
          invoke('read_file_base64', { path: tab.path }),
        ]);
        if (cancelled) return;

        const bytes = base64ToBytes(res.data);
        // The editor's `documentBuffer` prop accepts ArrayBuffer; pass
        // the underlying buffer of the Uint8Array view so we don't
        // copy the bytes again.
        setBuffer(bytes.buffer);
        setEditorComponent(() => editorMod.DocxEditor);
        // Flip the dirty gate open on the next task tick — by then the
        // editor's initial `onChange` burst from loading the document
        // will have flushed.
        setTimeout(() => { readyForDirtyRef.current = true; }, 0);
      } catch (e) {
        if (!cancelled) {
          const msg = e?.message || String(e);
          setError(`Couldn't open document: ${msg}`);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [tab.path, reloadVersion]);

  // Mirror dirty into the editor store so the tab gets a yellow dot.
  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, tab.id]);

  // ── Workaround for the eigenpal "scroll-closes-dropdown" bug ───
  // The library's `ve` hook (used by FontSizePicker and similar
  // hand-rolled popups) registers `window.addEventListener('scroll',
  // closeDropdown, true)` to close the popup on outside scroll. Their
  // listener fires for ANY scroll in the window — including scrolling
  // inside the dropdown's own scrollable list. So as soon as you
  // touch the dropdown's scrollbar, the popup closes.
  //
  // We can't patch their listener directly, but we can install a
  // capture-phase listener BEFORE they do (mount-time, vs.
  // dropdown-open-time) and `stopImmediatePropagation` for scroll
  // events whose target is inside a `[role="listbox"]`. Same capture
  // phase on the same window node — calling stopImmediatePropagation
  // here prevents the eigenpal listener (added later, also capture)
  // from firing for the same event.
  //
  // The check is narrow: only scrolls *inside* a listbox get blocked,
  // so the normal "close on outside scroll" UX still works for clicks
  // / scrolls anywhere else in the app.
  useEffect(() => {
    const onScrollCapture = (e) => {
      const t = e.target;
      if (!t || !(t instanceof Element)) return;
      if (t.closest?.('[role="listbox"]')) {
        e.stopImmediatePropagation();
      }
    };
    window.addEventListener('scroll', onScrollCapture, true);
    return () => window.removeEventListener('scroll', onScrollCapture, true);
  }, []);

  const onSave = async () => {
    const ref = editorRef.current;
    if (!ref || saving) return;
    setSaving(true);
    try {
      // `save()` returns an ArrayBuffer of the serialised .docx. Pass
      // `selective: false` to force a full repack — selective re-uses
      // unchanged parts of the original archive, which is faster but
      // can confuse the file watcher because the resulting bytes are
      // not deterministic across saves. Full repack matches what Word
      // does on save and round-trips cleanly.
      const out = await ref.save({ selective: false });
      if (!out) throw new Error('Editor returned no buffer');
      const b64 = bytesToBase64(new Uint8Array(out));
      await invoke('write_file_base64', { path: tab.path, data: b64 });
      setDirty(false);
      toast.success('Saved');
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Save failed: ${msg}`);
    } finally {
      setSaving(false);
    }
  };

  // Register Ctrl+S handler while this preview is active.
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

  if (!EditorComponent || !buffer) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <div className="flex flex-col gap-2">
          <Skeleton className="h-4 w-48" />
          <Skeleton className="h-[600px] w-[480px]" />
        </div>
      </div>
    );
  }

  return (
    <div className="rustic-docx-host flex h-full w-full flex-col">
      <EditorComponent
        ref={editorRef}
        documentBuffer={buffer}
        onChange={() => {
          // Initial load fires a burst of `onChange` calls; gate so
          // opening a file doesn't immediately mark it dirty.
          if (!readyForDirtyRef.current) return;
          setDirty(true);
        }}
        onError={(e) => {
          // eslint-disable-next-line no-console
          console.error('docx-editor error', e);
          toast.error(`Editor error: ${e?.message || e}`);
        }}
        showToolbar
        showZoomControl
        className="h-full w-full"
      />
    </div>
  );
}
