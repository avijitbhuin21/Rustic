import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { invoke } from '@tauri-apps/api/core';
import { fetchFileBytes, fetchFileRange, fileSize } from '@/lib/file-bytes';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Pencil,
  MousePointer2,
  Type,
  Highlighter,
  RotateCw,
  Trash2,
  Undo2,
  Save,
  Shapes,
  TextCursor,
} from 'lucide-react';
import { PanZoomControls, PanZoomSurface, usePanZoom } from './pan-zoom';
import { PdfObjectLayer, usePdfObjects } from './pdf-objects';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';
import { cn } from '@/lib/utils';
import { MOD_LABEL } from '@/lib/platform';
import './pdf-preview.css';

// PDFs up to this size load whole (fast, and save keeps the bytes); larger
// ones stream through pdf.js range requests.
const SMALL_PDF_BYTES = 16 * 1024 * 1024;

// PDF.js singleton loader. We import the bundled worker URL through Vite so
// it gets fingerprinted and served from /assets; this avoids the cross-origin
// worker errors that pdf.js's default CDN loader hits inside a Tauri webview.
let pdfjsPromise = null;
function loadPdfJs() {
  if (!pdfjsPromise) {
    pdfjsPromise = import('pdfjs-dist').then(async (pdfjs) => {
      const workerModule = await import('pdfjs-dist/build/pdf.worker.min.mjs?url');
      pdfjs.GlobalWorkerOptions.workerSrc = workerModule.default;
      return pdfjs;
    });
  }
  return pdfjsPromise;
}

// pdf-lib is heavier than we want on the read-only path, so it's only loaded
// once the user enters edit mode or saves.
let pdfLibPromise = null;
function loadPdfLib() {
  if (!pdfLibPromise) pdfLibPromise = import('pdf-lib');
  return pdfLibPromise;
}

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

function newId() {
  return Math.random().toString(36).slice(2, 10);
}

// Convert a #rrggbb string into the 0..1 triple pdf-lib expects.
function hexToRgb01(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return { r: 0, g: 0, b: 0 };
  const n = parseInt(m[1], 16);
  return {
    r: ((n >> 16) & 0xff) / 255,
    g: ((n >> 8) & 0xff) / 255,
    b: (n & 0xff) / 255,
  };
}

// Per-page edit shape:
//   { texts: [], highlights: [], rotation: 0|90|180|270, deleted: boolean }
// Texts/highlights live in PDF user-space coords (origin = bottom-left, y up,
// units = points). Storing them this way makes the pdf-lib mapping at save
// time a no-op — no inverse transforms, no scale-dependent math.
function emptyPageEdit() {
  return { texts: [], highlights: [], rotation: 0, deleted: false };
}
function isPageEmpty(edit) {
  if (!edit) return true;
  return (
    edit.texts.length === 0 &&
    edit.highlights.length === 0 &&
    edit.rotation === 0 &&
    !edit.deleted
  );
}

// One canvas per page. Rendering happens lazily when the canvas scrolls into
// view (IntersectionObserver fans out to the doc's getPage / render). Until
// then we hold a sized placeholder of the right aspect ratio so the scroll
// position doesn't jump as pages stream in.
function PdfPageCanvas({
  doc,
  pdfjs,
  pageNumber,
  scale,
  baseViewport,
  edit,
  editMode,
  tool,
  textDefaults,
  highlightDefaults,
  onEditChange,
  onPageAction,
  objects,
}) {
  const wrapRef = useRef(null);
  const canvasRef = useRef(null);
  const overlayRef = useRef(null);
  const textLayerRef = useRef(null);
  const renderTaskRef = useRef(null);
  // pdf.js TextLayer instance for the current scale. Held so we can call
  // .cancel() on scale change / unmount; otherwise concurrent renders
  // race and leave stale spans in the DOM.
  const textLayerInstanceRef = useRef(null);
  const [visible, setVisible] = useState(false);
  const [rendered, setRendered] = useState(false);
  // Active drag for highlight tool. We store screen-space coords during the
  // drag and translate to PDF user-space only when the gesture commits.
  const [drag, setDrag] = useState(null);
  // The text edit currently being typed. Stored as id; we look it up in the
  // edit list to render an autofocused contenteditable on top of the canvas.
  const [editingTextId, setEditingTextId] = useState(null);

  // Width/height at the requested scale, computed from the cached base
  // viewport (taken at scale=1). Doing it without an extra getPage roundtrip
  // means every page reserves correct space before its bytes are decoded.
  const dims = useMemo(() => {
    if (!baseViewport) return { width: 0, height: 0 };
    return {
      width: Math.max(1, Math.floor(baseViewport.width * scale)),
      height: Math.max(1, Math.floor(baseViewport.height * scale)),
    };
  }, [baseViewport, scale]);

  // Lazy render + release. Pages within 1500px of the viewport render; pages
  // that leave that band drop their bitmap so a 400+ page document only ever
  // holds the handful of canvases near what you're looking at.
  useEffect(() => {
    const node = wrapRef.current;
    if (!node) return;
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setVisible(e.isIntersecting);
      },
      { rootMargin: '1500px 1500px', threshold: 0 },
    );
    io.observe(node);
    return () => io.disconnect();
  }, [pageNumber]);

  useEffect(() => {
    if (visible) return;
    const canvas = canvasRef.current;
    if (canvas) {
      canvas.width = 0;
      canvas.height = 0;
    }
    textLayerRef.current?.replaceChildren();
    setRendered(false);
  }, [visible]);

  // Re-render whenever scale changes (canvas pixel size must match the
  // scaled viewport for the bitmap to stay sharp at zoom levels).
  // Also re-renders the text layer overlay so the user can select/copy
  // page text — without it, the page reads as a flat image.
  useEffect(() => {
    if (!visible || !doc) return;
    let cancelled = false;
    (async () => {
      try {
        const pdfPage = await doc.getPage(pageNumber);
        if (cancelled) return;
        // Use device pixel ratio so retina displays don't render fuzzy, but
        // cap the backing store: deep zoom on a big page would otherwise ask
        // for a multi-hundred-MB bitmap. Past the cap CSS upscales.
        const dpr = Math.min(window.devicePixelRatio || 1, 2);
        const base = pdfPage.getViewport({ scale: 1 });
        const MAX_CANVAS_PX = 16_000_000;
        const pxScale = Math.min(scale * dpr, Math.sqrt(MAX_CANVAS_PX / (base.width * base.height)));
        const viewport = pdfPage.getViewport({ scale: pxScale });
        const canvas = canvasRef.current;
        if (!canvas) return;
        // Render off-screen, then copy: resizing the live canvas would blank
        // it for the whole render and flash white after every zoom.
        const off = document.createElement('canvas');
        off.width = Math.floor(viewport.width);
        off.height = Math.floor(viewport.height);
        if (renderTaskRef.current) {
          try { renderTaskRef.current.cancel(); } catch {}
        }
        const task = pdfPage.render({ canvasContext: off.getContext('2d'), viewport });
        renderTaskRef.current = task;
        await task.promise;
        if (cancelled) return;
        canvas.width = off.width;
        canvas.height = off.height;
        canvas.getContext('2d').drawImage(off, 0, 0);
        off.width = 0;
        off.height = 0;
        setRendered(true);

        // Text layer overlay. The viewport here is at the *display* scale
        // (no DPR multiplier) because the layer is sized in CSS pixels —
        // it overlays the canvas's CSS size, not its backing-store size.
        const textLayerContainer = textLayerRef.current;
        if (pdfjs?.TextLayer && textLayerContainer && !cancelled) {
          // Cancel any previous render before tearing down its DOM, then
          // wipe the container so we don't stack spans from prior scales.
          if (textLayerInstanceRef.current) {
            try { textLayerInstanceRef.current.cancel(); } catch {}
            textLayerInstanceRef.current = null;
          }
          textLayerContainer.replaceChildren();
          textLayerContainer.style.setProperty('--total-scale-factor', String(scale));
          const textViewport = pdfPage.getViewport({ scale });
          const textContentSource = pdfPage.streamTextContent
            ? pdfPage.streamTextContent({ includeMarkedContent: true })
            : await pdfPage.getTextContent();
          if (cancelled) return;
          const layer = new pdfjs.TextLayer({
            textContentSource,
            container: textLayerContainer,
            viewport: textViewport,
          });
          textLayerInstanceRef.current = layer;
          try {
            await layer.render();
          } catch (e) {
            // pdf.js throws "TextLayer task cancelled" when we cancel mid-
            // render; that's expected during fast zoom and not a real error.
            if (!String(e?.message || e).toLowerCase().includes('cancel')) throw e;
          }
        }
      } catch (e) {
        if (e?.name !== 'RenderingCancelledException') {
          // eslint-disable-next-line no-console
          console.error('PdfPageCanvas render failed', pageNumber, e);
        }
      }
    })();
    return () => {
      cancelled = true;
      if (renderTaskRef.current) {
        try { renderTaskRef.current.cancel(); } catch {}
      }
      if (textLayerInstanceRef.current) {
        try { textLayerInstanceRef.current.cancel(); } catch {}
        textLayerInstanceRef.current = null;
      }
    };
  }, [doc, pdfjs, pageNumber, scale, visible, dims.width, dims.height]);

  // Screen px (relative to overlay) → PDF user-space coords. The overlay div
  // shares the wrap's dimensions, so `relY=0` is the top of the page.
  const screenToPdf = useCallback(
    (relX, relY) => {
      const baseH = baseViewport?.height || 0;
      return {
        xPdf: relX / scale,
        yPdf: baseH - relY / scale,
      };
    },
    [baseViewport, scale],
  );

  // Mouse handlers on the overlay div. The overlay only intercepts events when
  // edit mode is on AND a creative tool is selected; otherwise pointer-events
  // is none and clicks fall through to the page beneath (which lets users
  // scroll/select normally).
  const onOverlayMouseDown = (e) => {
    if (!editMode) return;
    if (edit?.deleted) return;
    const rect = overlayRef.current.getBoundingClientRect();
    // The view is CSS-transformed (pan/zoom), so screen px ≠ layout px.
    const f = overlayRef.current.offsetWidth / (rect.width || 1);
    const relX = (e.clientX - rect.left) * f;
    const relY = (e.clientY - rect.top) * f;

    if (tool === 'text') {
      // Place a new text box at the click. Width is a sensible default that
      // the user can tweak later; height matches the chosen font size at
      // ~1.4x line height so the input feels right.
      const { xPdf, yPdf } = screenToPdf(relX, relY);
      const fontSize = textDefaults.fontSize;
      const heightPdf = fontSize * 1.4;
      const widthPdf = 220;
      const id = newId();
      const next = {
        ...(edit || emptyPageEdit()),
        texts: [
          ...((edit || emptyPageEdit()).texts),
          {
            id,
            xPdf,
            // Store the *bottom* edge; the text element grows downward
            // from the click position visually.
            yBottomPdf: yPdf - heightPdf,
            widthPdf,
            heightPdf,
            fontSize,
            color: textDefaults.color,
            text: '',
          },
        ],
      };
      onEditChange(next);
      setEditingTextId(id);
      e.preventDefault();
      e.stopPropagation();
      return;
    }

    if (tool === 'highlight') {
      setDrag({ startX: relX, startY: relY, curX: relX, curY: relY });
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const onOverlayMouseMove = (e) => {
    if (!drag) return;
    const rect = overlayRef.current.getBoundingClientRect();
    const f = overlayRef.current.offsetWidth / (rect.width || 1);
    setDrag({
      ...drag,
      curX: Math.max(0, Math.min(overlayRef.current.offsetWidth, (e.clientX - rect.left) * f)),
      curY: Math.max(0, Math.min(overlayRef.current.offsetHeight, (e.clientY - rect.top) * f)),
    });
  };

  const onOverlayMouseUp = () => {
    if (!drag) return;
    const w = Math.abs(drag.curX - drag.startX);
    const h = Math.abs(drag.curY - drag.startY);
    setDrag(null);
    if (w < 4 || h < 4) return; // ignore stray clicks

    const leftPx = Math.min(drag.startX, drag.curX);
    const topPx = Math.min(drag.startY, drag.curY);
    const { xPdf, yPdf: topPdf } = screenToPdf(leftPx, topPx);
    const widthPdf = w / scale;
    const heightPdf = h / scale;
    const next = {
      ...(edit || emptyPageEdit()),
      highlights: [
        ...((edit || emptyPageEdit()).highlights),
        {
          id: newId(),
          xPdf,
          yBottomPdf: topPdf - heightPdf,
          widthPdf,
          heightPdf,
          color: highlightDefaults.color,
          opacity: highlightDefaults.opacity,
        },
      ],
    };
    onEditChange(next);
  };

  // Commit / cancel for an in-flight text edit. We bind on the document so
  // clicks outside the textarea finalise the value, matching how PowerPoint
  // / Google Docs handle text-box editing.
  useEffect(() => {
    if (!editingTextId) return;
    const onDocClick = (e) => {
      const ta = overlayRef.current?.querySelector(`[data-text-id="${editingTextId}"] textarea`);
      if (ta && !ta.contains(e.target)) {
        setEditingTextId(null);
      }
    };
    document.addEventListener('mousedown', onDocClick, true);
    return () => document.removeEventListener('mousedown', onDocClick, true);
  }, [editingTextId]);

  // Objects tool: ask for this page's objects once it is near the viewport, and
  // again whenever an edit has made the previous answer stale.
  const objectsOn = editMode && tool === 'objects' && !edit?.deleted;
  const objectState = objects.pages[pageNumber];
  const requestObjects = objects.requestPage;
  useEffect(() => {
    if (objectsOn && visible && !objectState) requestObjects(pageNumber);
  }, [objectsOn, visible, objectState, requestObjects, pageNumber]);

  const baseH = baseViewport?.height || 0;
  const overlayActive = editMode && (tool === 'text' || tool === 'highlight') && !edit?.deleted;

  return (
    <div
      ref={wrapRef}
      data-pdf-page={pageNumber}
      style={{ width: dims.width, height: dims.height }}
      className={cn(
        'group relative shrink-0 bg-white shadow-lg',
        edit?.deleted && 'opacity-30',
      )}
    >
      <canvas ref={canvasRef} className="block h-full w-full" />
      {/* Selectable text overlay. Sits above the canvas (z-index from
          CSS) but BELOW the edit overlay below, so when the edit overlay
          is `pointer-events-none` (default / select tool), text-layer
          spans receive the pointer events and the user can drag-select. */}
      <div ref={textLayerRef} className="rustic-pdf-textlayer" aria-hidden="true" />
      {objectsOn && (
        <PdfObjectLayer
          pageNumber={pageNumber}
          scale={scale}
          pageHeight={baseH}
          state={objectState}
          busy={objects.busy}
          onApply={objects.apply}
          onUndo={objects.undo}
        />
      )}
      {!rendered && !canvasRef.current?.width && (
        <div className="absolute inset-0 flex items-center justify-center text-xs text-muted-foreground">
          <span>{pageNumber}</span>
        </div>
      )}

      {/* Edit overlay. Renders existing edits + captures input. */}
      <div
        ref={overlayRef}
        onMouseDown={onOverlayMouseDown}
        onMouseMove={onOverlayMouseMove}
        onMouseUp={onOverlayMouseUp}
        onMouseLeave={onOverlayMouseUp}
        className={cn(
          'absolute inset-0',
          overlayActive ? 'cursor-crosshair' : 'pointer-events-none',
          tool === 'text' && overlayActive && 'cursor-text',
        )}
        style={{ touchAction: 'none' }}
      >
        {/* Existing highlights */}
        {(edit?.highlights || []).map((h) => {
          const left = h.xPdf * scale;
          const top = (baseH - h.yBottomPdf - h.heightPdf) * scale;
          const width = h.widthPdf * scale;
          const height = h.heightPdf * scale;
          return (
            <div
              key={h.id}
              className="absolute"
              style={{
                left,
                top,
                width,
                height,
                background: h.color,
                opacity: h.opacity,
                mixBlendMode: 'multiply',
                pointerEvents: editMode ? 'auto' : 'none',
              }}
              onMouseDown={(e) => {
                // Alt-click to delete an existing highlight. Without an
                // explicit modifier this would conflict with the highlight
                // *creation* gesture (start of a new drag).
                if (e.altKey) {
                  e.stopPropagation();
                  const next = {
                    ...edit,
                    highlights: edit.highlights.filter((x) => x.id !== h.id),
                  };
                  onEditChange(next);
                }
              }}
              title={editMode ? 'Alt+click to delete highlight' : ''}
            />
          );
        })}

        {/* Existing texts */}
        {(edit?.texts || []).map((t) => {
          const left = t.xPdf * scale;
          const top = (baseH - t.yBottomPdf - t.heightPdf) * scale;
          const width = t.widthPdf * scale;
          const height = t.heightPdf * scale;
          const isEditing = editingTextId === t.id;
          return (
            <div
              key={t.id}
              data-text-id={t.id}
              className={cn(
                'absolute select-none',
                editMode && !isEditing && 'cursor-text',
              )}
              style={{
                left,
                top,
                width,
                height,
                color: t.color,
                fontSize: t.fontSize * scale,
                lineHeight: `${t.heightPdf * scale}px`,
                fontFamily: 'Helvetica, Arial, sans-serif',
                pointerEvents: editMode ? 'auto' : 'none',
              }}
              onMouseDown={(e) => {
                if (!editMode) return;
                e.stopPropagation();
                if (e.altKey) {
                  // Alt-click deletes
                  const next = {
                    ...edit,
                    texts: edit.texts.filter((x) => x.id !== t.id),
                  };
                  onEditChange(next);
                  return;
                }
                setEditingTextId(t.id);
              }}
            >
              {isEditing ? (
                <textarea
                  autoFocus
                  defaultValue={t.text}
                  className="h-full w-full resize-none border border-dashed border-info bg-white/80 p-0 text-inherit outline-none"
                  style={{
                    fontSize: t.fontSize * scale,
                    lineHeight: `${t.heightPdf * scale}px`,
                    fontFamily: 'inherit',
                    color: 'inherit',
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Escape') {
                      setEditingTextId(null);
                      e.preventDefault();
                    } else if (e.key === 'Enter' && !e.shiftKey) {
                      // Commit on Enter; Shift+Enter inserts a newline.
                      e.currentTarget.blur();
                      e.preventDefault();
                    }
                  }}
                  onBlur={(e) => {
                    const value = e.currentTarget.value;
                    if (!value.trim()) {
                      // Empty text → drop the placeholder edit entirely.
                      const next = {
                        ...edit,
                        texts: edit.texts.filter((x) => x.id !== t.id),
                      };
                      onEditChange(next);
                    } else if (value !== t.text) {
                      const next = {
                        ...edit,
                        texts: edit.texts.map((x) =>
                          x.id === t.id ? { ...x, text: value } : x,
                        ),
                      };
                      onEditChange(next);
                    }
                    setEditingTextId(null);
                  }}
                />
              ) : (
                <div
                  className={cn(
                    'h-full w-full whitespace-pre-wrap',
                    editMode &&
                      'rounded-sm outline outline-1 outline-transparent hover:outline-info',
                  )}
                >
                  {t.text || <span className="opacity-40">click to edit</span>}
                </div>
              )}
            </div>
          );
        })}

        {/* In-flight highlight drag preview */}
        {drag && (
          <div
            className="pointer-events-none absolute border border-yellow-600 bg-yellow-300/40"
            style={{
              left: Math.min(drag.startX, drag.curX),
              top: Math.min(drag.startY, drag.curY),
              width: Math.abs(drag.curX - drag.startX),
              height: Math.abs(drag.curY - drag.startY),
            }}
          />
        )}
      </div>

      {/* Per-page action chips (visible in edit mode). Kept on top via
          z-index 10 so they're clickable even when a highlight sits beneath. */}
      {editMode && (
          <div className="absolute right-2 top-2 z-10 flex items-center gap-1 rounded-md bg-background/90 px-1 py-0.5 shadow ring-1 ring-border opacity-0 transition-opacity group-hover:opacity-100 touch-reveal">
          <span className="px-1 text-[10px] text-muted-foreground">p{pageNumber}</span>
          <Button
            size="icon-xs"
            variant="ghost"
            title={`Rotate (queued: ${edit?.rotation || 0}°)`}
            onClick={() => onPageAction('rotate', pageNumber)}
          >
            <RotateCw />
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            title={edit?.deleted ? 'Restore page' : 'Delete page'}
            onClick={() => onPageAction('toggleDelete', pageNumber)}
          >
            <Trash2 />
          </Button>
          {!isPageEmpty(edit) && (
            <Button
              size="icon-xs"
              variant="ghost"
              title="Undo all edits on this page"
              onClick={() => onPageAction('reset', pageNumber)}
            >
              <Undo2 />
            </Button>
          )}
        </div>
      )}

      {/* Status badges (rotation queued / deletion queued) — always visible
          so the user can see queued state even outside hover. */}
      {(edit?.rotation || edit?.deleted) && (
        <div className="pointer-events-none absolute left-2 top-2 z-10 flex gap-1">
          {edit.rotation ? (
            <span className="rounded bg-info/90 px-1.5 py-0.5 text-[10px] font-medium text-on-status">
              rot {edit.rotation}°
            </span>
          ) : null}
          {edit.deleted ? (
            <span className="rounded bg-destructive/90 px-1.5 py-0.5 text-[10px] font-medium text-white">
              will delete
            </span>
          ) : null}
        </div>
      )}
    </div>
  );
}

export default function PdfPreview({ tab }) {
  const [doc, setDoc] = useState(null);
  // Held alongside `doc` so PdfPageCanvas can call `new pdfjs.TextLayer(...)`
  // for the selectable text overlay — the loaded `PDFDocumentProxy` doesn't
  // expose the module itself, and re-importing per-page would add latency.
  const [pdfjsMod, setPdfjsMod] = useState(null);
  const [pageBaseViewports, setPageBaseViewports] = useState([]);
  // Original PDF bytes — held so save can re-open the original through pdf-lib
  // even after pdf.js has the doc cached. Re-reading from disk would work too
  // but introduces a race with any in-flight overlays the user just authored.
  const [originalBytes, setOriginalBytes] = useState(null);
  const [error, setError] = useState(null);
  // Resolution the pages are laid out and rendered at. Live zoom is a GPU
  // transform on top (usePanZoom); when a gesture settles the pages are
  // re-rendered at the new resolution and the transform is rebased.
  const [renderScale, setRenderScale] = useState(1);
  const renderScaleRef = useRef(1);
  const pendingRebaseRef = useRef(1);
  const [currentPage, setCurrentPage] = useState(1);
  const [textSelect, setTextSelect] = useState(false);
  const [loadNote, setLoadNote] = useState(null);

  // Edit-mode state. `edits` is a per-page map keyed by 1-based page number.
  const [editMode, setEditMode] = useState(false);
  const [tool, setTool] = useState('select'); // 'select' | 'objects' | 'text' | 'highlight'
  const [edits, setEdits] = useState({});
  const [textColor, setTextColor] = useState('#000000');
  const [textSize, setTextSize] = useState(14);
  const [highlightColor, setHighlightColor] = useState('#fff176');
  const [saving, setSaving] = useState(false);

  const lastReportedPageRef = useRef(1);

  const tabSetDirty = useEditor((s) => s.setDirty);

  // Object edits (move / delete / retype existing content) are applied by the
  // backend; after each one pdf.js is handed the edited document to render.
  const originalBytesRef = useRef(null);
  originalBytesRef.current = originalBytes;
  const showBytes = useCallback(async (bytes) => {
    /** Shows `bytes`, or the file as it is on disk when `bytes` is null. */
    const pdfjs = await loadPdfJs();
    const source = bytes ?? originalBytesRef.current ?? (await fetchFileBytes(tab.path));
    // pdf.js may detach the buffer it is given; keep ours intact for save.
    setDoc(await pdfjs.getDocument({ data: source.slice() }).promise);
  }, [tab.path]);
  const objects = usePdfObjects({ path: tab.path, onDocBytes: showBytes });
  const resetObjects = objects.reset;

  const overlayDirty = useMemo(() => {
    for (const k of Object.keys(edits)) {
      if (!isPageEmpty(edits[k])) return true;
    }
    return false;
  }, [edits]);
  const dirty = overlayDirty || objects.count > 0;

  // Mirror local dirty state into the editor store so the tab gets a yellow
  // dot. Clear on unmount.
  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, tab.id]);

  const reloadVersion = useFileReloadVersion(tab.path, { enabled: !dirty });

  // Load doc + base viewports for every page in one pass. Holding the base
  // viewports lets every PdfPageCanvas reserve scroll space without an extra
  // async getPage round-trip on first paint.
  //
  // `signal` is a `{ cancelled: boolean }` flag the caller flips on unmount /
  // dep change. We re-check it after every await so a superseded load never
  // commits its (now-stale) `loaded` doc into React state — otherwise React's
  // StrictMode double-mount in dev hands the canvases a destroyed
  // PDFDocumentProxy whose worker transport is null, and the next `getPage`
  // throws `Cannot read properties of null (reading 'sendWithPromise')`.
  const loadDoc = useCallback(async (signal = { cancelled: false }) => {
    setError(null);
    setDoc(null);
    setPageBaseViewports([]);
    setOriginalBytes(null);
    setEdits({});
    resetObjects();
    setLoadNote(null);

    try {
      const [pdfjs, total] = await Promise.all([loadPdfJs(), fileSize(tab.path, reloadVersion)]);
      if (signal.cancelled) return null;
      let task;
      if (total <= SMALL_PDF_BYTES) {
        const bytes = await fetchFileBytes(tab.path, { version: reloadVersion });
        if (signal.cancelled) return null;
        setOriginalBytes(bytes);
        // pdf.js may transfer (detach) the buffer it's given; keep ours intact for save.
        task = pdfjs.getDocument({ data: bytes.slice() });
      } else {
        // Big document: pdf.js asks for byte ranges as pages need them, so a
        // multi-hundred-MB PDF opens without being read whole.
        const initial = await fetchFileRange(tab.path, 0, Math.min(total, 1 << 20), { version: reloadVersion });
        if (signal.cancelled) return null;
        const transport = new pdfjs.PDFDataRangeTransport(total, initial);
        transport.requestDataRange = (begin, end) => {
          fetchFileRange(tab.path, begin, end, { version: reloadVersion })
            .then((chunk) => transport.onDataRange(begin, chunk))
            .catch((err) => setError(`Failed reading PDF: ${String(err?.message || err)}`));
        };
        task = pdfjs.getDocument({
          range: transport,
          rangeChunkSize: 1 << 20,
          disableAutoFetch: true,
          disableStream: true,
        });
      }
      const loaded = await task.promise;
      if (signal.cancelled) {
        try { loaded.destroy(); } catch {}
        return null;
      }
      // Lay every page out at the first page's size immediately, then fill
      // in the real sizes in the background — opening a 400+ page document
      // must not wait on 400 getPage round trips.
      const first = (await loaded.getPage(1)).getViewport({ scale: 1 });
      if (signal.cancelled) {
        try { loaded.destroy(); } catch {}
        return null;
      }
      const viewports = Array.from({ length: loaded.numPages }, () => first);
      setDoc(loaded);
      setPdfjsMod(pdfjs);
      setPageBaseViewports(viewports);
      (async () => {
        const BATCH = 24;
        for (let start = 2; start <= loaded.numPages; start += BATCH) {
          const end = Math.min(loaded.numPages, start + BATCH - 1);
          const batch = await Promise.all(
            Array.from({ length: end - start + 1 }, (_, i) =>
              loaded.getPage(start + i).then((p) => p.getViewport({ scale: 1 })),
            ),
          ).catch(() => null);
          if (signal.cancelled || !batch) return;
          let changed = false;
          batch.forEach((v, i) => {
            const idx = start - 1 + i;
            if (v.width !== viewports[idx].width || v.height !== viewports[idx].height) changed = true;
            viewports[idx] = v;
          });
          if (changed) setPageBaseViewports([...viewports]);
        }
      })();
      if (total > SMALL_PDF_BYTES) {
        setLoadNote(`${(total / 1048576).toFixed(0)} MB · streamed`);
      }
      return loaded;
    } catch (e) {
      if (!signal.cancelled) setError(String(e?.message || e));
      return null;
    }
  }, [tab.path, reloadVersion, resetObjects]);

  useEffect(() => {
    const signal = { cancelled: false };
    let docRef = null;
    (async () => {
      docRef = await loadDoc(signal);
    })();
    return () => {
      signal.cancelled = true;
      if (docRef) {
        try { docRef.destroy(); } catch {}
      }
    };
  }, [loadDoc]);

  // Page layout in render-scale-1 units. Gaps and padding scale with the
  // pages, so the whole layout grows linearly with renderScale — that's what
  // lets a re-render be absorbed by rebasing the transform with no jump.
  const layout = useMemo(() => {
    const PAD = 16;
    const GAP = 12;
    const offsets = [];
    let y = PAD;
    let maxW = 0;
    for (const v of pageBaseViewports) {
      offsets.push(y);
      y += v.height + GAP;
      maxW = Math.max(maxW, v.width);
    }
    return { PAD, GAP, offsets, width: maxW + PAD * 2, height: y - GAP + PAD };
  }, [pageBaseViewports]);

  const updateCurrentPage = useCallback((view, r) => {
    const vp = pzRef.current?.viewportRef.current;
    if (!vp || layout.offsets.length === 0) return;
    const centerY = (vp.clientHeight / 2 - view.y) / view.k / r;
    let n = 1;
    for (let i = 0; i < layout.offsets.length; i++) {
      if (layout.offsets[i] <= centerY) n = i + 1;
      else break;
    }
    if (n !== lastReportedPageRef.current) {
      lastReportedPageRef.current = n;
      setCurrentPage(n);
    }
  }, [layout]);

  const pz = usePanZoom({
    fit: 'width',
    wheel: 'scroll',
    padding: 0,
    minScale: 0.01,
    maxScale: 200,
    // With the objects tool, a drag that starts on an object moves it; anywhere
    // else it pans as usual.
    canPan: (e) =>
      !textSelect &&
      (!editMode ||
        tool === 'select' ||
        (tool === 'objects' && !e?.target?.closest?.('[data-pdf-object],[data-pdf-object-editor]'))),
    onSettle: (view) => {
      const r = renderScaleRef.current;
      updateCurrentPage(view, r);
      // Re-render at the resolution actually on screen (clamped: tiny
      // thumbnails and extreme zoom both fall back to CSS scaling).
      const target = Math.min(8, Math.max(0.1, r * view.k));
      if (Math.abs(target / r - 1) > 0.03) {
        pendingRebaseRef.current = target / r;
        renderScaleRef.current = target;
        setRenderScale(target);
      }
    },
  });
  const pzRef = useRef(pz);
  pzRef.current = pz;

  useLayoutEffect(() => {
    if (pendingRebaseRef.current !== 1) {
      pz.rebase(pendingRebaseRef.current);
      pendingRebaseRef.current = 1;
    }
    if (layout.offsets.length) pz.setContentSize(layout.width * renderScale, layout.height * renderScale);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renderScale, layout]);

  // Cleanup the loaded doc on unmount or path swap.
  useEffect(() => {
    return () => {
      if (doc) {
        try { doc.destroy(); } catch {}
      }
    };
  }, [doc]);

  const jumpToPage = (n) => {
    const top = layout.offsets[n - 1];
    if (top == null) return;
    const v = pz.getView();
    pz.setView({ y: -(top - layout.PAD / 2) * renderScale * v.k });
    lastReportedPageRef.current = n;
  };

  // Update a single page's edits. The callback drops the entry entirely if it
  // ends up empty so `dirty` stays accurate without bookkeeping.
  const onEditChange = useCallback((pageNumber, next) => {
    setEdits((prev) => {
      const out = { ...prev };
      if (isPageEmpty(next)) {
        delete out[pageNumber];
      } else {
        out[pageNumber] = next;
      }
      return out;
    });
  }, []);

  const onPageAction = useCallback((kind, pageNumber) => {
    setEdits((prev) => {
      const out = { ...prev };
      const current = prev[pageNumber] || emptyPageEdit();
      let next = current;
      if (kind === 'rotate') {
        next = { ...current, rotation: (current.rotation + 90) % 360 };
      } else if (kind === 'toggleDelete') {
        next = { ...current, deleted: !current.deleted };
      } else if (kind === 'reset') {
        next = emptyPageEdit();
      }
      if (isPageEmpty(next)) delete out[pageNumber];
      else out[pageNumber] = next;
      return out;
    });
  }, []);

  const undoAll = () => {
    setEdits({});
    if (objects.count > 0) {
      resetObjects();
      showBytes(null).catch((e) => setError(String(e?.message || e)));
    }
  };

  // --------------------------------------------------------------- save flow
  // We open the *original* bytes through pdf-lib (not the modified pdf.js
  // doc — pdf.js doesn't expose mutated bytes), apply every queued edit on
  // top, write back via the existing write_file_base64 command, then reload
  // the pdf.js doc so the user sees their changes baked in.
  const onSave = useCallback(async () => {
    if (saving || !doc) return;
    if (!dirty) {
      toast.message('No changes to save');
      return;
    }
    setSaving(true);
    try {
      // Object edits are already baked into the document the backend returned.
      // With no overlays on top there is nothing for pdf-lib to add, and the
      // backend writes the file itself (no size cap, unlike write_file_base64).
      const edited = objects.bytes();
      if (edited && !overlayDirty) {
        await objects.save();
        toast.success('Saved');
        await loadDoc();
        return;
      }
      const { PDFDocument, StandardFonts, rgb, degrees } = await loadPdfLib();
      // Streamed (large) documents never held their bytes; read them now.
      const source = edited ?? originalBytes ?? (await fetchFileBytes(tab.path));
      const pdfDoc = await PDFDocument.load(source);
      const helvetica = await pdfDoc.embedFont(StandardFonts.Helvetica);
      const pages = pdfDoc.getPages();

      // Apply texts + highlights + rotations first. We do deletions in a
      // *separate* second pass because removePage shifts subsequent indices,
      // and we want all the index math here to refer to the source doc.
      for (let i = 0; i < pages.length; i++) {
        const pageNumber = i + 1;
        const edit = edits[pageNumber];
        if (!edit) continue;
        const page = pages[i];

        for (const h of edit.highlights) {
          const { r, g, b } = hexToRgb01(h.color);
          page.drawRectangle({
            x: h.xPdf,
            y: h.yBottomPdf,
            width: h.widthPdf,
            height: h.heightPdf,
            color: rgb(r, g, b),
            opacity: h.opacity ?? 0.4,
          });
        }

        for (const t of edit.texts) {
          if (!t.text) continue;
          const { r, g, b } = hexToRgb01(t.color);
          // pdf-lib's drawText positions the baseline at `y`. We stored the
          // bottom edge of the visual element; nudging up by ~20% of the
          // font size lands the baseline inside the element where the user
          // saw the text on screen.
          const baselineY = t.yBottomPdf + t.fontSize * 0.2;
          page.drawText(t.text, {
            x: t.xPdf,
            y: baselineY,
            size: t.fontSize,
            font: helvetica,
            color: rgb(r, g, b),
            maxWidth: t.widthPdf,
            lineHeight: t.fontSize * 1.2,
          });
        }

        if (edit.rotation) {
          // Rotations compose with whatever the page's existing rotation
          // was (some PDFs ship with non-zero rotation already).
          const existing = page.getRotation().angle || 0;
          page.setRotation(degrees((existing + edit.rotation) % 360));
        }
      }

      // Now delete pages. Walk back-to-front so each removePage doesn't
      // invalidate the indices of pages we still need to delete.
      const toDelete = Object.keys(edits)
        .map((k) => Number(k))
        .filter((n) => edits[n]?.deleted)
        .sort((a, b) => b - a);
      for (const pageNumber of toDelete) {
        if (pageNumber >= 1 && pageNumber <= pdfDoc.getPageCount()) {
          pdfDoc.removePage(pageNumber - 1);
        }
      }

      if (pdfDoc.getPageCount() === 0) {
        throw new Error('Refusing to save: all pages were deleted.');
      }

      const outBytes = await pdfDoc.save();
      const b64 = bytesToBase64(outBytes);
      await invoke('write_file_base64', { path: tab.path, data: b64 });
      toast.success('Saved');

      // Reload from disk so pdf.js shows the now-baked-in edits and our
      // edits map starts fresh.
      await loadDoc();
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Save failed: ${msg}`);
    } finally {
      setSaving(false);
    }
  }, [dirty, overlayDirty, objects, doc, edits, originalBytes, saving, tab.path, loadDoc]);

  // Register Ctrl+S handler while we're the active editor.
  useEffect(() => {
    setActiveSaver(onSave);
    return () => clearActiveSaver(onSave);
  }, [onSave]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (!doc || pageBaseViewports.length === 0) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-[400px] w-[300px]" />
      </div>
    );
  }

  const toolbar = (
    <>
      <div className="flex items-center gap-1.5">
        <Input
          type="number"
          min={1}
          max={pageBaseViewports.length}
          value={currentPage}
          onChange={(e) => {
            const n = Math.max(1, Math.min(pageBaseViewports.length, Number(e.target.value) || 1));
            setCurrentPage(n);
            jumpToPage(n);
          }}
          className="h-6 w-16 text-xs tabular-nums"
        />
        <span className="text-xs text-muted-foreground">
          / {pageBaseViewports.length}
        </span>

        {/* Edit toggle */}
        <div className="mx-2 h-4 w-px bg-border" />
        <Button
          size="xs"
          variant={editMode ? 'default' : 'ghost'}
          onClick={() => {
            setEditMode((v) => !v);
            setTool('select');
          }}
        >
          <Pencil />
          <span className="ml-1">{editMode ? 'Editing' : 'Edit'}</span>
        </Button>

        {/* Edit tools, shown only in edit mode */}
        {editMode && (
          <>
            <div className="ml-1 flex items-center rounded-md border border-border bg-muted/30">
              <Button
                size="icon-xs"
                variant={tool === 'select' ? 'default' : 'ghost'}
                title="Select / scroll"
                onClick={() => setTool('select')}
              >
                <MousePointer2 />
              </Button>
              <Button
                size="icon-xs"
                variant={tool === 'objects' ? 'default' : 'ghost'}
                title="Edit page content: drag to move, Delete to remove, double-click text to retype"
                aria-label="Edit page content"
                onClick={() => setTool('objects')}
              >
                <Shapes />
              </Button>
              <Button
                size="icon-xs"
                variant={tool === 'text' ? 'default' : 'ghost'}
                title="Add text"
                onClick={() => setTool('text')}
              >
                <Type />
              </Button>
              <Button
                size="icon-xs"
                variant={tool === 'highlight' ? 'default' : 'ghost'}
                title="Highlight"
                onClick={() => setTool('highlight')}
              >
                <Highlighter />
              </Button>
            </div>

            {tool === 'text' && (
              <div className="ml-1 flex items-center gap-1">
                <input
                  type="color"
                  value={textColor}
                  onChange={(e) => setTextColor(e.target.value)}
                  title="Text color"
                  className="h-5 w-6 cursor-pointer rounded border border-border bg-transparent"
                />
                <Input
                  type="number"
                  min={6}
                  max={96}
                  value={textSize}
                  onChange={(e) => setTextSize(Math.max(6, Math.min(96, Number(e.target.value) || 14)))}
                  className="h-6 w-12 text-xs"
                  title="Font size (pt)"
                />
              </div>
            )}
            {tool === 'highlight' && (
              <div className="ml-1 flex items-center gap-1">
                <input
                  type="color"
                  value={highlightColor}
                  onChange={(e) => setHighlightColor(e.target.value)}
                  title="Highlight color"
                  className="h-5 w-6 cursor-pointer rounded border border-border bg-transparent"
                />
              </div>
            )}

            {objects.count > 0 && (
              <Button
                size="xs"
                variant="ghost"
                disabled={objects.busy}
                onClick={() => objects.undo()}
                title="Undo the last content edit"
              >
                <Undo2 />
                <span className="ml-1">Undo</span>
              </Button>
            )}
            {dirty && (
              <Button size="xs" variant="ghost" onClick={undoAll} title="Discard all edits">
                <Undo2 />
                <span className="ml-1">Reset</span>
              </Button>
            )}
            <Button
              size="xs"
              variant={dirty ? 'default' : 'ghost'}
              disabled={!dirty || saving}
              onClick={onSave}
              title={`Save (${MOD_LABEL}+S)`}
            >
              <Save />
              <span className="ml-1">{saving ? 'Saving…' : 'Save'}</span>
            </Button>
          </>
        )}
      </div>
      <div className="flex items-center gap-1">
        {loadNote && <span className="mr-1 text-[11px] text-muted-foreground">{loadNote}</span>}
        <Button
          size="icon-xs"
          variant={textSelect ? 'default' : 'ghost'}
          onClick={() => setTextSelect((v) => !v)}
          title={textSelect ? 'Text selection on — drag selects text' : 'Select text (drag pans otherwise)'}
          aria-label="Toggle text selection"
        >
          <TextCursor />
        </Button>
        <PanZoomControls pz={pz} fitLabel="Fit width" />
      </div>
    </>
  );

  const textDefaults = { color: textColor, fontSize: textSize };
  const highlightDefaults = { color: highlightColor, opacity: 0.4 };

  return (
    <PanZoomSurface
      pz={pz}
      toolbar={toolbar}
      background="bg-muted/30"
      contentClassName={textSelect ? 'select-text cursor-text' : ''}
    >
      <div
        className="flex flex-col items-center"
        style={{
          width: layout.width * renderScale,
          padding: layout.PAD * renderScale,
          gap: layout.GAP * renderScale,
        }}
      >
        {pageBaseViewports.map((bv, idx) => {
          const pageNumber = idx + 1;
          return (
            <PdfPageCanvas
              key={pageNumber}
              doc={doc}
              pdfjs={pdfjsMod}
              pageNumber={pageNumber}
              scale={renderScale}
              baseViewport={bv}
              edit={edits[pageNumber]}
              editMode={editMode}
              tool={tool}
              textDefaults={textDefaults}
              highlightDefaults={highlightDefaults}
              onEditChange={(next) => onEditChange(pageNumber, next)}
              onPageAction={onPageAction}
              objects={objects}
            />
          );
        })}
      </div>
    </PanZoomSurface>
  );
}
