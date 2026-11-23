import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronDown, Maximize2, Menu, TableOfContents, ZoomIn, ZoomOut } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { MOD_LABEL } from '@/lib/platform';
import { fetchFileBytes } from '@/lib/file-bytes';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';
import { extractImagesFromClipboard, readImagesFromAsyncClipboard } from '@/lib/clipboard-image';
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
 * Wheel scrolls the pages; pinch or Cmd/Ctrl+wheel = zoom (around the cursor),
 * drag on the background / middle-drag / Space+drag = pan. Live zoom is a GPU transform on the
 * page area; when the gesture settles we commit once through the editor's
 * own setZoom (which re-lays the document out crisply) and re-anchor the
 * scroll so the point under the cursor stays put. Dragging on text still
 * selects it, so editing keeps working.
 */
function useDocxPanZoom(hostRef, editorRef, ready, onZoom) {
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
      onZoom?.(z1);
      // Two frames: let the editor re-render at the new zoom, then re-anchor.
      requestAnimationFrame(() => requestAnimationFrame(() => {
        scroll.scrollLeft = docX * z1 - g.cx;
        scroll.scrollTop = docY * z1 - g.cy;
      }));
    };

    const onWheel = (e) => {
      if (!scroll || !scroll.contains(e.target)) return;
      // Plain wheel / two-finger swipe is left alone so the pages scroll natively.
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      e.stopPropagation();
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
      // Pinch sends small ctrl+wheel deltas; a mouse wheel notch sends ~100, so cap the step.
      const step = -dy * (e.ctrlKey ? 0.012 : 0.0022);
      gesture.factor *= Math.exp(Math.max(-0.25, Math.min(0.25, step)));
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
  }, [hostRef, editorRef, ready, onZoom]);
}

/** Clamps a docx zoom factor to the range the editor lays out well. */
function clampDocxZoom(z) {
  return Math.min(4, Math.max(0.25, z));
}

// Desk kept on each side of a page.
const DOCX_GUTTER = 24;

/** Unscaled page width plus any extra width the editor reserves (open sidebar), or null before layout. */
function docxLayoutMetrics(host) {
  const scroll = host?.querySelector('.docx-editor__scroll-container');
  const page = host?.querySelector('.layout-page');
  const frame = scroll?.firstElementChild;
  if (!scroll || !frame || !page?.offsetWidth) return null;
  // The editor's own minimum is page + 2 × 20px (outline button off); whatever exceeds that is sidebar room.
  const reserved = parseFloat(frame.style.minWidth) || 0;
  return { scroll, pageWidth: page.offsetWidth, extra: Math.max(0, reserved - page.offsetWidth - 40) };
}

/** Zoom at which one page spans the visible width of the docx scroll area, or null before layout. */
function docxFitWidthZoom(host) {
  const m = docxLayoutMetrics(host);
  if (!m) return null;
  return clampDocxZoom((m.scroll.clientWidth - 2 * DOCX_GUTTER - m.extra) / m.pageWidth);
}

/**
 * Keeps the docx layout as wide as the zoomed page plus its gutters. The
 * editor fixes that width for 100% and scales the page about its centre, so
 * a zoomed-in page lost its left edge and a fitted page sat off-centre.
 */
function useDocxLayoutWidth(hostRef, zoom, ready) {
  useEffect(() => {
    const host = hostRef.current;
    if (!ready || !host) return undefined;
    let frameMo = null;
    const sync = () => {
      const m = docxLayoutMetrics(host);
      if (!m) return false;
      host.style.setProperty('--docx-layout-min', `${Math.round(m.pageWidth * zoom + 2 * DOCX_GUTTER + m.extra)}px`);
      if (!frameMo) {
        frameMo = new MutationObserver(sync);
        frameMo.observe(m.scroll.firstElementChild, { attributes: true, attributeFilter: ['style'] });
      }
      return true;
    };
    let hostMo = null;
    if (!sync()) {
      hostMo = new MutationObserver(() => { if (sync()) hostMo.disconnect(); });
      hostMo.observe(host, { childList: true, subtree: true });
    }
    return () => {
      hostMo?.disconnect();
      frameMo?.disconnect();
    };
  }, [hostRef, zoom, ready]);
}

/**
 * Holds a freshly opened document at its top while the editor settles. After
 * a zoom change the editor restores a scroll anchor on each layout pass, which
 * left a just-fitted document a few dozen pixels down. Ends on the first user
 * input or after 2.5s; returns a function that ends it early.
 */
function holdDocxAtTop(scroll) {
  const inputs = ['wheel', 'pointerdown', 'keydown', 'touchstart'];
  const onScroll = () => { if (scroll.scrollTop !== 0) scroll.scrollTop = 0; };
  let timer = 0;
  const stop = () => {
    clearTimeout(timer);
    scroll.removeEventListener('scroll', onScroll);
    for (const type of inputs) window.removeEventListener(type, stop, true);
  };
  scroll.addEventListener('scroll', onScroll);
  for (const type of inputs) window.addEventListener(type, stop, true);
  timer = setTimeout(stop, 2500);
  onScroll();
  return stop;
}

/** Single toolbar button that reveals the editor's File / Format / Insert / Help menus. */
function DocxMenuButton({ open, onToggle }) {
  return (
    <button
      type="button"
      className="rustic-docx-menu-btn"
      aria-label="Document menu"
      aria-expanded={open}
      title="File, Format, Insert, Help"
      onClick={onToggle}
    >
      <Menu />
    </button>
  );
}

/** Chevron that shows or hides the toolbar controls that did not fit on the first row. */
function DocxToolbarOverflowButton({ expanded, onToggle }) {
  return (
    <button
      type="button"
      className="rustic-docx-menu-btn rustic-docx-more-btn"
      aria-label={expanded ? 'Show fewer tools' : 'Show more tools'}
      aria-expanded={expanded}
      title={expanded ? 'Show fewer tools' : 'Show more tools'}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onToggle}
    >
      <ChevronDown />
    </button>
  );
}

/** Floating zoom out / percentage / zoom in / fit-width control for the docx page area. */
function DocxZoomControl({ zoom, onZoom, onFit }) {
  return (
    <div className="rustic-docx-zoom absolute bottom-3 right-6 z-20 flex items-center gap-0.5 rounded-md border border-border bg-popover/95 p-0.5 shadow-sm">
      <Button size="icon-xs" variant="ghost" onClick={() => onZoom(zoom / 1.15)} aria-label="Zoom out" title={`Zoom out (${MOD_LABEL}+scroll or pinch)`}>
        <ZoomOut />
      </Button>
      <button
        type="button"
        className="w-11 text-center text-xs tabular-nums text-muted-foreground hover:text-foreground"
        onClick={() => onZoom(1)}
        title="Reset to 100%"
      >
        {Math.round(zoom * 100)}%
      </button>
      <Button size="icon-xs" variant="ghost" onClick={() => onZoom(zoom * 1.15)} aria-label="Zoom in" title={`Zoom in (${MOD_LABEL}+scroll or pinch)`}>
        <ZoomIn />
      </Button>
      <Button size="icon-xs" variant="ghost" onClick={onFit} aria-label="Fit width" title="Fit width">
        <Maximize2 />
      </Button>
    </div>
  );
}

/** Headings of the open document as `{ text, level, pos }`, by outline level or a `HeadingN` style (the editor's own rule). */
function collectDocxHeadings(doc) {
  const headings = [];
  doc?.descendants((node, pos) => {
    if (node.type.name !== 'paragraph') return;
    let level = node.attrs.outlineLevel;
    if (level == null) {
      const match = /^[Hh]eading(\d)$/.exec(node.attrs.styleId || '');
      if (match) level = Number(match[1]) - 1;
    }
    if (level == null || level < 0 || level > 8) return;
    let text = '';
    node.forEach((child) => { if (child.isText) text += child.text || ''; });
    if (text.trim()) headings.push({ text: text.trim(), level, pos });
  });
  return headings;
}

/** Floating bottom-left button that lists the document's headings; picking one jumps to it and the list stays open. */
function DocxContentsButton({ editorRef }) {
  const [open, setOpen] = useState(false);
  const [headings, setHeadings] = useState([]);
  const collect = () => collectDocxHeadings(editorRef.current?.getEditorRef?.()?.getView?.()?.state.doc);
  const onOpenChange = (next) => {
    if (next) setHeadings(collect());
    setOpen(next);
  };
  const jump = (index) => {
    // The list stays open while the document is edited, so positions are re-read at jump time.
    const fresh = collect();
    const wanted = headings[index];
    const target = fresh[index]?.text === wanted.text ? fresh[index] : fresh.find((h) => h.text === wanted.text) || wanted;
    setHeadings(fresh);
    const paged = editorRef.current?.getEditorRef?.();
    paged?.scrollToPosition?.(target.pos);
    paged?.setSelection?.(target.pos + 1);
    paged?.focus?.();
  };
  const top = headings.length ? Math.min(...headings.map((h) => h.level)) : 0;
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <div className="rustic-docx-contents absolute bottom-3 left-6 z-20 flex items-center rounded-md border border-border bg-popover/95 p-0.5 shadow-sm">
        <PopoverTrigger asChild>
          <Button size="icon-xs" variant={open ? 'secondary' : 'ghost'} aria-label="Table of contents" title="Table of contents">
            <TableOfContents />
          </Button>
        </PopoverTrigger>
      </div>
      <PopoverContent
        side="top"
        align="start"
        sideOffset={8}
        className="w-72 gap-1 p-1.5"
        // Keep the caret in the document instead of returning focus to the button.
        onCloseAutoFocus={(e) => e.preventDefault()}
        // The list behaves like a panel: jumping, clicking or typing in the
        // document leaves it open. Only its button and Escape close it.
        onInteractOutside={(e) => e.preventDefault()}
      >
        <div className="px-2 pt-1 text-xs font-medium text-muted-foreground">Contents</div>
        {headings.length === 0 ? (
          <div className="px-2 py-3 text-xs text-muted-foreground">This document has no headings.</div>
        ) : (
          <div className="max-h-[min(60vh,420px)] overflow-y-auto">
            {headings.map((h, index) => (
              <button
                key={h.pos}
                type="button"
                title={h.text}
                onClick={() => jump(index)}
                style={{ paddingLeft: 8 + (h.level - top) * 12 }}
                className={`block w-full truncate rounded-md py-1 pr-2 text-left text-xs hover:bg-accent hover:text-accent-foreground ${h.level === top ? 'font-medium text-foreground' : 'text-muted-foreground'}`}
              >
                {h.text}
              </button>
            ))}
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}

/**
 * Image paste fallback for the docx editor. The editor inserts images that
 * arrive as files on the paste event; this covers pastes that carry nothing
 * usable (its own right-click Paste forwards text only, and WebKit can expose
 * a copied image solely through the async clipboard) by reading the image
 * from the async clipboard and replaying it as a file paste.
 */
function useDocxImagePaste(editorRef, ready) {
  useEffect(() => {
    if (!ready) return undefined;
    // The editor mounts its ProseMirror element under <body>, not inside our
    // host, so the event is matched against this editor's own views instead.
    const ownView = (target) => {
      const paged = editorRef.current?.getEditorRef?.();
      const views = [paged?.getView?.(), ...(paged?.getHfPmViews?.()?.values?.() || [])];
      return views.find((v) => v?.dom?.contains(target))?.dom || null;
    };
    const onPaste = async (e) => {
      const pm = ownView(e.target);
      if (!pm) return;
      const data = e.clipboardData;
      const hasImageFile = extractImagesFromClipboard(data).length > 0
        || Array.from(data?.files || []).some((f) => f.type.startsWith('image/'));
      if (hasImageFile) return;
      if (Array.from(data?.types || []).some((t) => t !== 'Files')) return;
      e.preventDefault();
      e.stopPropagation();
      const images = await readImagesFromAsyncClipboard();
      if (!images.length) return;
      const transfer = new DataTransfer();
      for (const { file } of images) transfer.items.add(file);
      pm.dispatchEvent(new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }));
    };
    document.addEventListener('paste', onPaste, true);
    return () => document.removeEventListener('paste', onPaste, true);
  }, [editorRef, ready]);
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

  const ready = !!EditorComponent && !!buffer;
  const [menuOpen, setMenuOpen] = useState(false);
  const [zoom, setZoom] = useState(1);

  const applyZoom = useCallback((z) => {
    const next = clampDocxZoom(z);
    editorRef.current?.setZoom?.(next);
    setZoom(next);
  }, []);
  const fitWidth = useCallback(() => {
    const fit = docxFitWidthZoom(hostRef.current);
    if (fit) applyZoom(fit);
  }, [applyZoom]);

  useDocxPanZoom(hostRef, editorRef, ready, setZoom);
  useDocxLayoutWidth(hostRef, zoom, ready);
  useDocxImagePaste(editorRef, ready);

  // A page wider than the pane opens fitted to its width instead of cut off
  // on the right. Runs once per file; it never enlarges a page that fits.
  const autoFitRef = useRef(null);
  useEffect(() => {
    const host = hostRef.current;
    if (!ready || !host || autoFitRef.current === tab.path) return undefined;
    let release = null;
    const tryFit = () => {
      const fit = docxFitWidthZoom(host);
      if (fit == null) return false;
      autoFitRef.current = tab.path;
      if (fit < 1) {
        applyZoom(fit);
        const scroll = host.querySelector('.docx-editor__scroll-container');
        if (scroll) release = holdDocxAtTop(scroll);
      }
      return true;
    };
    if (tryFit()) return () => release?.();
    const mo = new MutationObserver(() => { if (tryFit()) mo.disconnect(); });
    mo.observe(host, { childList: true, subtree: true });
    return () => {
      mo.disconnect();
      release?.();
    };
  }, [ready, tab.path, applyZoom]);

  // Keep the readout right if a zoom key handled elsewhere changes the editor's zoom.
  useEffect(() => {
    if (!ready) return undefined;
    const onKey = (e) => {
      if (!(e.ctrlKey || e.metaKey) || !['+', '=', '-', '0'].includes(e.key)) return;
      requestAnimationFrame(() => {
        const z = editorRef.current?.getZoom?.();
        if (z) setZoom(z);
      });
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [ready]);

  // The toolbar is one row; when its groups wrap (narrow pane, or the extra
  // table tools) a chevron offers the rest.
  const [toolbarOverflow, setToolbarOverflow] = useState(false);
  const [toolbarExpanded, setToolbarExpanded] = useState(false);
  useEffect(() => {
    const host = hostRef.current;
    if (!ready || !host) return undefined;
    let bar = null;
    let barMo = null;
    // One row is 24px tall; anything taller means groups wrapped.
    const measure = () => { if (bar) setToolbarOverflow(bar.scrollHeight > 30); };
    const attach = () => {
      bar = host.querySelector('[data-testid="formatting-bar"]');
      if (!bar) return false;
      barMo = new MutationObserver(measure);
      barMo.observe(bar, { childList: true, subtree: true });
      measure();
      return true;
    };
    let hostMo = null;
    if (!attach()) {
      hostMo = new MutationObserver(() => { if (attach()) hostMo.disconnect(); });
      hostMo.observe(host, { childList: true, subtree: true });
    }
    const ro = new ResizeObserver(measure);
    ro.observe(host);
    return () => {
      hostMo?.disconnect();
      barMo?.disconnect();
      ro.disconnect();
    };
  }, [ready]);
  const toolbarOpen = toolbarOverflow && toolbarExpanded;

  // The menu strip closes on Escape, on a press outside it, and once a menu item has run.
  useEffect(() => {
    if (!menuOpen) return undefined;
    const host = hostRef.current;
    const onDown = (e) => {
      if (!host?.querySelector('[data-testid="title-bar"]')?.contains(e.target)) setMenuOpen(false);
    };
    const onKey = (e) => { if (e.key === 'Escape') setMenuOpen(false); };
    const onClick = (e) => {
      const menubar = host?.querySelector('[data-testid="title-bar"] [role="menubar"]');
      if (!menubar?.contains(e.target)) return;
      // A menu name only opens its list; a submenu parent leaves the list open.
      if (e.target.closest('button')?.parentElement?.parentElement === menubar) return;
      requestAnimationFrame(() => {
        if (!menubar.querySelector(':scope > div > div')) setMenuOpen(false);
      });
    };
    document.addEventListener('pointerdown', onDown, true);
    document.addEventListener('keydown', onKey);
    document.addEventListener('click', onClick);
    return () => {
      document.removeEventListener('pointerdown', onDown, true);
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('click', onClick);
    };
  }, [menuOpen]);

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
    <div
      ref={hostRef}
      className="rustic-docx-host flex h-full w-full flex-col"
      data-menu-open={menuOpen ? '' : undefined}
      data-toolbar-overflow={toolbarOverflow ? '' : undefined}
      data-toolbar-expanded={toolbarOpen ? '' : undefined}
    >
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
        showZoomControl={false}
        showOutlineButton={false}
        initialZoom={zoom}
        renderLogo={() => <DocxMenuButton open={menuOpen} onToggle={() => setMenuOpen((v) => !v)} />}
        className="h-full w-full"
      />
      {toolbarOverflow && (
        <DocxToolbarOverflowButton expanded={toolbarOpen} onToggle={() => setToolbarExpanded((v) => !v)} />
      )}
      <DocxContentsButton editorRef={editorRef} />
      <DocxZoomControl zoom={zoom} onZoom={applyZoom} onFit={fitWidth} />
    </div>
  );
}
