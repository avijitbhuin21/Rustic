// Object editing for the PDF preview: move, delete and retype what is already on
// a page (images and lines of text), as opposed to the overlay tools, which draw
// new text and highlights on top.
//
// The backend (`pdf_edit`) is stateless. We keep the list of edits and send the
// whole list on every call; it reloads the file, replays the list and returns
// the page's objects plus the edited document, which pdf.js then renders. Undo
// is the same call with the last entry left out.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { X } from 'lucide-react';
import { cn } from '@/lib/utils';

function base64ToBytes(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function errorText(e) {
  return typeof e === 'string' ? e : e?.message || String(e);
}

/** Replays `groups` of edits on the file; returns the page's objects and, when asked, the edited PDF. */
function pdfEdit(path, groups, page, wantPdf) {
  return invoke('pdf_edit', { path, ops: groups.flat(), page, wantPdf });
}

/** Tells the user what the engine could not do exactly as asked. */
function reportNotes(state, count) {
  const notes = (state.notes || []).slice(-count);
  const missing = [...new Set(notes.map((n) => n.missing || '').join(''))].join('');
  if (missing) toast.warning(`No available font has “${missing}”. Those characters were left out.`);
  if (notes.some((n) => n.overlaps)) toast.message('The edited text now runs into other content.');
  if (notes.some((n) => n.clipped)) toast.message('Part of the edit is hidden by a clipping area on the page.');
}

/**
 * Edit list and per-page object state for one PDF.
 * `onDocBytes(bytes | null)` must show the given document (null: the file as it is on disk).
 */
export function usePdfObjects({ path, onDocBytes }) {
  const [groups, setGroups] = useState([]);
  const [pages, setPages] = useState({});
  const [busy, setBusy] = useState(false);
  const live = useRef({ groups: [], version: 0, busy: false, pending: new Set(), bytes: null, warned: false });
  const onDocBytesRef = useRef(onDocBytes);
  onDocBytesRef.current = onDocBytes;

  const reset = useCallback(() => {
    const l = live.current;
    l.groups = [];
    l.version += 1;
    l.pending.clear();
    l.bytes = null;
    setGroups([]);
    setPages({});
  }, []);

  const requestPage = useCallback(
    (page) => {
      const l = live.current;
      const key = `${l.version}:${page}`;
      if (l.pending.has(key)) return;
      l.pending.add(key);
      const version = l.version;
      pdfEdit(path, l.groups, page, false)
        .then((res) => res.state, (e) => ({ error: errorText(e) }))
        .then((state) => {
          if (live.current.version !== version) return;
          setPages((prev) => (prev[page] ? prev : { ...prev, [page]: state }));
        });
    },
    [path],
  );

  const run = useCallback(
    async (next, page) => {
      const l = live.current;
      if (l.busy) return null;
      l.busy = true;
      setBusy(true);
      try {
        const res = await pdfEdit(path, next, page, next.length > 0);
        const bytes = res.pdf ? base64ToBytes(res.pdf) : null;
        await onDocBytesRef.current?.(bytes);
        l.groups = next;
        l.version += 1;
        l.pending.clear();
        l.bytes = bytes;
        setGroups(next);
        setPages({ [page]: res.state });
        return res.state;
      } catch (e) {
        toast.error(`PDF edit failed: ${errorText(e)}`);
        return null;
      } finally {
        l.busy = false;
        setBusy(false);
      }
    },
    [path],
  );

  const apply = useCallback(
    async (page, ops) => {
      const l = live.current;
      const state = await run([...l.groups, ops.map((op) => ({ ...op, page }))], page);
      if (!state) return false;
      if (state.info?.signed && !l.warned) {
        l.warned = true;
        toast.warning('This PDF is digitally signed. Saving this edit will invalidate the signature.');
      }
      reportNotes(state, ops.length);
      return true;
    },
    [run],
  );

  const undo = useCallback(async () => {
    const l = live.current;
    if (!l.groups.length) return false;
    const page = l.groups[l.groups.length - 1][0].page;
    return !!(await run(l.groups.slice(0, -1), page));
  }, [run]);

  const bytes = useCallback(() => live.current.bytes, []);

  /** Writes the file with every edit applied. The backend replays the list, so the document is not sent back. */
  const save = useCallback(() => invoke('pdf_edit_save', { path, ops: live.current.groups.flat() }), [path]);

  return { count: groups.length, pages, busy, requestPage, apply, undo, reset, bytes, save };
}

/** The things a user can grab on a page: every image, and every line of text. */
function unitsOf(state) {
  if (!state || state.error) return [];
  const images = state.objects.objects
    .filter((o) => o.kind === 'image')
    .map((o) => ({ key: `i${o.id}`, kind: 'image', ids: [o.id], bbox: o.bbox }));
  const lines = state.layout.blocks
    .flatMap((b) => b.lines)
    .map((l) => ({ key: `t${l.id}`, kind: 'text', ids: l.runs, lineId: l.id, bbox: l.bbox, text: l.text.trimEnd(), size: l.size }));
  // Text after images, so a line over a picture is the one that gets the click.
  return [...images, ...lines];
}

/**
 * Selectable boxes over one page. Coordinates from the backend are PDF points
 * with the origin at the bottom-left of the page as displayed.
 */
export function PdfObjectLayer({ pageNumber, scale, pageHeight, state, busy, onApply, onUndo }) {
  const [selected, setSelected] = useState(null);
  // `drag` is what is drawn; where the gesture began lives in a ref so that the
  // pointer handlers never read a render that is one event behind.
  const [drag, setDrag] = useState(null);
  const dragStart = useRef(null);
  const rootRef = useRef(null);
  const [editing, setEditing] = useState(null);
  const editingRef = useRef(null);
  editingRef.current = editing;
  const units = useMemo(() => unitsOf(state), [state]);

  // New geometry has arrived: the box is where the object now is.
  useEffect(() => setDrag(null), [state]);

  const remove = (u) => onApply(pageNumber, u.ids.map((id) => ({ op: 'delete', id })));
  const move = (u, dx, dy) => onApply(pageNumber, u.ids.map((id) => ({ op: 'move', id, dx, dy })));

  const onDown = (e, u) => {
    if (e.button !== 0 || busy || editing) return;
    const el = e.currentTarget;
    const root = rootRef.current;
    const width = root?.getBoundingClientRect().width;
    setSelected(u.key);
    el.focus({ preventScroll: true });
    try { el.setPointerCapture(e.pointerId); } catch { /* not capturable */ }
    // The view is CSS-transformed (pan/zoom), so screen px are not layout px.
    // Measured on the page, not the object: offsetWidth is rounded to whole px.
    dragStart.current = { key: u.key, x: e.clientX, y: e.clientY, f: width ? root.offsetWidth / width : 1 };
    setDrag({ key: u.key, dx: 0, dy: 0 });
  };

  /** Distance of the pointer from where the drag began, in layout px. */
  const offsetAt = (e) => {
    const s = dragStart.current;
    return s ? { dx: (e.clientX - s.x) * s.f, dy: (e.clientY - s.y) * s.f } : null;
  };

  const onMove = (e) => {
    const off = offsetAt(e);
    if (off) setDrag({ key: dragStart.current.key, ...off });
  };

  const onUp = async (e, u) => {
    const off = offsetAt(e);
    dragStart.current = null;
    if (!off) return;
    if (Math.hypot(off.dx, off.dy) < 3) {
      setDrag(null);
      return;
    }
    // Keep the outline where it was dropped until the page has been redrawn.
    setDrag({ key: u.key, ...off });
    if (!(await move(u, off.dx / scale, -off.dy / scale))) setDrag(null);
  };

  const commitText = async () => {
    const ed = editingRef.current;
    if (!ed) return;
    editingRef.current = null;
    setEditing(null);
    const u = units.find((x) => x.key === ed.key);
    if (!u || ed.text === u.text) return;
    if (ed.text.trim()) await onApply(pageNumber, [{ op: 'replace_line', id: u.lineId, text: ed.text }]);
    else await remove(u);
  };

  // Keys for the focused object. The listener sits on window in the capture phase,
  // ahead of the app's global shortcuts: there, Delete removes the file that is
  // selected in the explorer.
  const onKeyRef = useRef(null);
  onKeyRef.current = (e) => {
    const el = document.activeElement;
    if (!el?.dataset?.pdfObject || !rootRef.current?.contains(el)) return;
    const u = units.find((x) => x.key === el.dataset.unitKey);
    if (!u) return;
    const step = e.shiftKey ? 10 : 1;
    const nudge = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] }[e.key];
    let act;
    if (e.key === 'Delete' || e.key === 'Backspace') act = () => remove(u);
    else if (nudge) act = () => move(u, nudge[0], nudge[1]);
    else if (e.key === 'Enter' && u.kind === 'text') act = () => setEditing({ key: u.key, text: u.text });
    else if (e.key === 'Escape') act = () => setSelected(null);
    else if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === 'z') act = () => onUndo();
    else return;
    // Ours even while an edit is in flight: it must not fall through to the app.
    e.preventDefault();
    e.stopPropagation();
    if (!busy) act();
  };
  useEffect(() => {
    const onKey = (e) => onKeyRef.current?.(e);
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  if (!state) return null;
  const note = state.error
    ? `This page cannot be edited: ${state.error}`
    : !state.objects.text_editable && !units.some((u) => u.kind === 'text')
      ? 'No editable text on this page (scanned or drawn as outlines).'
      : null;

  return (
    <div ref={rootRef} className="pointer-events-none absolute inset-0 z-[5]" data-pdf-object-layer={pageNumber}>
      {units.map((u) => {
        const [x0, y0, x1, y1] = u.bbox;
        const active = selected === u.key;
        const offset = drag?.key === u.key ? drag : null;
        const box = {
          left: x0 * scale,
          top: (pageHeight - y1) * scale,
          width: Math.max(6, (x1 - x0) * scale),
          height: Math.max(6, (y1 - y0) * scale),
        };
        if (editing?.key === u.key) {
          return (
            <textarea
              key={u.key}
              autoFocus
              data-pdf-object-editor={u.lineId}
              value={editing.text}
              onChange={(e) => setEditing({ key: u.key, text: e.target.value.replace(/\n/g, ' ') })}
              onBlur={commitText}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === 'Enter') {
                  e.preventDefault();
                  commitText();
                } else if (e.key === 'Escape') {
                  editingRef.current = null;
                  setEditing(null);
                }
              }}
              className="pointer-events-auto absolute resize-none overflow-hidden whitespace-pre rounded-sm border border-primary bg-white px-0 py-0 text-black outline-none ring-2 ring-primary/30"
              style={{
                ...box,
                width: Math.max(box.width + u.size * scale * 4, 140),
                fontSize: u.size * scale,
                lineHeight: `${box.height}px`,
              }}
            />
          );
        }
        return (
          <div
            key={u.key}
            tabIndex={0}
            role="button"
            aria-label={u.kind === 'text' ? `Text: ${u.text}` : 'Image'}
            data-pdf-object={u.kind}
            data-object-id={u.ids[0]}
            data-unit-key={u.key}
            title={u.kind === 'text' ? 'Drag to move · double-click to edit · Delete to remove' : 'Drag to move · Delete to remove'}
            onPointerDown={(e) => onDown(e, u)}
            onPointerMove={onMove}
            onPointerUp={(e) => onUp(e, u)}
            onPointerCancel={() => {
              dragStart.current = null;
              setDrag(null);
            }}
            onDoubleClick={() => u.kind === 'text' && !busy && setEditing({ key: u.key, text: u.text })}
            className={cn(
              'pointer-events-auto absolute cursor-move rounded-[1px] border outline-none',
              active ? 'border-primary bg-primary/10 ring-1 ring-primary/40' : 'border-transparent hover:border-primary/60',
              busy && 'cursor-progress',
            )}
            style={{
              ...box,
              transform: offset ? `translate(${offset.dx}px, ${offset.dy}px)` : undefined,
              touchAction: 'none',
            }}
          >
            {active && !offset && (
              <button
                type="button"
                data-pdf-object-delete
                title="Delete"
                aria-label="Delete"
                onPointerDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  if (!busy) remove(u);
                }}
                className="absolute -right-2.5 -top-2.5 flex h-4 w-4 items-center justify-center rounded-full bg-destructive text-white shadow"
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </div>
        );
      })}
      {note && (
        <div className="absolute bottom-2 left-2 rounded bg-background/90 px-2 py-1 text-[11px] text-muted-foreground shadow ring-1 ring-border">
          {note}
        </div>
      )}
    </div>
  );
}
