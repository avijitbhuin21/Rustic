import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { fetchFileBytes } from '@/lib/file-bytes';
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

/**
 * Wheel = zoom (around the cursor), drag on the background / middle-drag /
 * Space+drag = pan, for the docx editor. Live zoom is a GPU transform on the
 * page area; when the gesture settles we commit once through the editor's
 * own setZoom (which re-lays the document out crisply) and re-anchor the
 * scroll so the point under the cursor stays put. Dragging on text still
 * selects it, so editing keeps working.
 */
function useDocxPanZoom(hostRef, editorRef, ready) {
  useEffect(() => {
    const host = hostRef.current;
    if (!host || !ready) return undefined;
    let scroll = null;
    let gesture = null; // { factor, cx, cy, ox, oy }
    let settle = 0;
    let space = false;
    let drag = null;
    const content = () => scroll?.firstElementChild;

    const commit = () => {
      const g = gesture;
      gesture = null;
      const el = content();
      if (!g || !el || !editorRef.current) return;
      const z0 = editorRef.current.getZoom?.() ?? 1;
      const z1 = Math.min(4, Math.max(0.25, z0 * g.factor));
      const docX = (scroll.scrollLeft + g.cx) / z0;
      const docY = (scroll.scrollTop + g.cy) / z0;
      el.style.transform = '';
      el.style.transformOrigin = '';
      el.style.willChange = '';
      editorRef.current.setZoom?.(z1);
      // Two frames: let the editor re-render at the new zoom, then re-anchor.
      requestAnimationFrame(() => requestAnimationFrame(() => {
        scroll.scrollLeft = docX * z1 - g.cx;
        scroll.scrollTop = docY * z1 - g.cy;
      }));
    };

    const onWheel = (e) => {
      if (!scroll || !scroll.contains(e.target)) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        scroll.scrollLeft += e.shiftKey ? 0 : e.deltaX;
        scroll.scrollTop += e.shiftKey ? e.deltaY : 0;
        return;
      }
      const rect = scroll.getBoundingClientRect();
      const cx = e.clientX - rect.left;
      const cy = e.clientY - rect.top;
      const el = content();
      if (!el) return;
      if (!gesture) {
        gesture = {
          factor: 1,
          cx,
          cy,
          ox: scroll.scrollLeft + cx - el.offsetLeft,
          oy: scroll.scrollTop + cy - el.offsetTop,
        };
        el.style.transformOrigin = `${gesture.ox}px ${gesture.oy}px`;
        el.style.willChange = 'transform';
      }
      const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
      gesture.factor *= Math.exp(-dy * (e.ctrlKey ? 0.012 : 0.0022));
      const z0 = editorRef.current?.getZoom?.() ?? 1;
      gesture.factor = Math.min(4 / z0, Math.max(0.25 / z0, gesture.factor));
      el.style.transform = `scale(${gesture.factor})`;
      clearTimeout(settle);
      settle = setTimeout(commit, 180);
    };

    // Anything in the scroll area that isn't the pages themselves is "desk".
    const isBackground = (t) => !t.closest?.('.paged-editor__pages');
    const onDown = (e) => {
      if (!scroll || !scroll.contains(e.target)) return;
      if (!(e.button === 1 || (e.button === 0 && (space || isBackground(e.target))))) return;
      e.preventDefault();
      drag = { x: e.clientX, y: e.clientY, l: scroll.scrollLeft, t: scroll.scrollTop, id: e.pointerId };
      scroll.setPointerCapture?.(e.pointerId);
      scroll.style.cursor = 'grabbing';
    };
    const onMove = (e) => {
      if (!drag || drag.id !== e.pointerId) return;
      scroll.scrollLeft = drag.l - (e.clientX - drag.x);
      scroll.scrollTop = drag.t - (e.clientY - drag.y);
    };
    const onUp = (e) => {
      if (!drag || drag.id !== e.pointerId) return;
      drag = null;
      scroll.style.cursor = '';
    };
    const onKey = (e) => {
      if (e.code !== 'Space') return;
      const typing = e.target?.isContentEditable || /INPUT|TEXTAREA/.test(e.target?.tagName || '');
      if (typing) return;
      space = e.type === 'keydown';
    };

    // The editor mounts its scroll container asynchronously.
    const attach = () => {
      scroll = host.querySelector('.docx-editor__scroll-container');
      return !!scroll;
    };
    let mo = null;
    if (!attach()) {
      mo = new MutationObserver(() => { if (attach()) mo.disconnect(); });
      mo.observe(host, { childList: true, subtree: true });
    }
    host.addEventListener('wheel', onWheel, { passive: false, capture: true });
    host.addEventListener('pointerdown', onDown, true);
    host.addEventListener('pointermove', onMove);
    host.addEventListener('pointerup', onUp);
    host.addEventListener('pointercancel', onUp);
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKey);
    return () => {
      mo?.disconnect();
      clearTimeout(settle);
      host.removeEventListener('wheel', onWheel, { capture: true });
      host.removeEventListener('pointerdown', onDown, true);
      host.removeEventListener('pointermove', onMove);
      host.removeEventListener('pointerup', onUp);
      host.removeEventListener('pointercancel', onUp);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('keyup', onKey);
    };
  }, [hostRef, editorRef, ready]);
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
  const hostRef = useRef(null);
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
        const [editorMod, _css, bytes] = await Promise.all([
          import('@eigenpal/docx-editor-react'),
          import('@eigenpal/docx-editor-react/styles.css'),
          fetchFileBytes(tab.path),
        ]);
        if (cancelled) return;

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

  useDocxPanZoom(hostRef, editorRef, !!EditorComponent && !!buffer);

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
    <div ref={hostRef} className="rustic-docx-host flex h-full w-full flex-col">
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
