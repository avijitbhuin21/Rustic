import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Copy, FolderOpen, ZoomIn, ZoomOut, Maximize2, Download,
} from 'lucide-react';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { toast } from 'sonner';
import { useAgent } from '@/state/agent';
import { revealInFileManager } from '@/state/explorer';
import { cn } from '@/lib/utils';

// ─── Format helpers ───────────────────────────────────────────────────────────

const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'avif', 'ico']);
const VIDEO_EXTS = new Set(['mp4', 'webm', 'mov', 'm4v', 'mkv', 'ogv']);

const IMAGE_MIME = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  webp: 'image/webp', bmp: 'image/bmp', avif: 'image/avif', ico: 'image/x-icon',
};
const VIDEO_MIME = {
  mp4: 'video/mp4', m4v: 'video/mp4', webm: 'video/webm', mov: 'video/quicktime',
  mkv: 'video/x-matroska', ogv: 'video/ogg',
};

function extOf(p) {
  if (!p) return '';
  const slashIdx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
  const name = slashIdx >= 0 ? p.slice(slashIdx + 1) : p;
  const dot = name.lastIndexOf('.');
  return dot < 0 ? '' : name.slice(dot + 1).toLowerCase();
}

function kindOf(p) {
  const e = extOf(p);
  if (IMAGE_EXTS.has(e)) return 'image';
  if (VIDEO_EXTS.has(e)) return 'video';
  return 'other';
}

function basenameOf(p) {
  if (!p) return '';
  const slashIdx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
  return slashIdx >= 0 ? p.slice(slashIdx + 1) : p;
}

// Join project root with a relative path emitted by the media tools. The Rust
// backend accepts mixed separators on Windows, so we don't bother normalising.
function joinAbs(root, rel) {
  if (!rel) return '';
  // Already absolute: forward as-is.
  if (/^[a-zA-Z]:[\\/]/.test(rel) || rel.startsWith('/') || rel.startsWith('\\\\')) {
    return rel;
  }
  if (!root) return rel;
  const sep = root.includes('\\') ? '\\' : '/';
  const trimmedRoot = root.replace(/[\\/]+$/, '');
  const trimmedRel = rel.replace(/^[\\/]+/, '');
  return `${trimmedRoot}${sep}${trimmedRel}`;
}

// ─── Public: parse the media-output block out of a tool result ────────────────

const MEDIA_BLOCK_RE = /```media-output\s*\n([\s\S]*?)\n```/;

/**
 * Extract the structured media-output payload from a tool's output text.
 * The Rust side (crates/rustic-agent/src/tools/media_tools.rs) wraps every
 * image/video/animate result in a fenced ```media-output block carrying
 * { tool, provider, model, mode, paths[], prompt, cost_usd }.
 */
export function parseMediaOutput(output) {
  if (typeof output !== 'string') return null;
  const m = output.match(MEDIA_BLOCK_RE);
  if (!m) return null;
  try {
    const data = JSON.parse(m[1]);
    if (!data || !Array.isArray(data.paths) || data.paths.length === 0) return null;
    return data;
  } catch {
    return null;
  }
}

/** Strip the ```media-output block from the displayable output text. */
export function stripMediaBlock(output) {
  if (typeof output !== 'string') return output;
  return output.replace(MEDIA_BLOCK_RE, '').trim();
}

// ─── Asset loader ─────────────────────────────────────────────────────────────
//
// We pull files through `read_file_base64`. For images that produces a data:
// URI that we can drop straight into <img src>. For videos we decode into a
// Blob and hand out an object URL — data URIs make Chromium re-parse the
// entire base64 string on every seek, which is unusable for clips of any
// length. Each loader is per-absolute-path and cached during the component's
// lifetime so reopening the lightbox doesn't re-read the file.

function b64ToBlob(b64, type) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new Blob([bytes], { type });
}

function useMediaSrc(absPath, kind) {
  const [state, setState] = useState({ src: null, error: null, loading: true });
  const blobUrlRef = useRef(null);
  useEffect(() => {
    let cancelled = false;
    setState({ src: null, error: null, loading: true });
    if (!absPath) {
      setState({ src: null, error: 'No path', loading: false });
      return () => { cancelled = true; };
    }
    invoke('read_file_base64', { path: absPath })
      .then((res) => {
        if (cancelled) return;
        const ext = extOf(absPath);
        if (kind === 'image') {
          const mime = IMAGE_MIME[ext] || 'image/png';
          setState({ src: `data:${mime};base64,${res.data}`, error: null, loading: false });
        } else if (kind === 'video') {
          const mime = VIDEO_MIME[ext] || 'video/mp4';
          const blob = b64ToBlob(res.data, mime);
          const url = URL.createObjectURL(blob);
          blobUrlRef.current = url;
          setState({ src: url, error: null, loading: false });
        } else {
          setState({ src: null, error: 'Unsupported media type', loading: false });
        }
      })
      .catch((e) => {
        if (cancelled) return;
        setState({ src: null, error: String(e), loading: false });
      });
    return () => {
      cancelled = true;
      if (blobUrlRef.current) {
        URL.revokeObjectURL(blobUrlRef.current);
        blobUrlRef.current = null;
      }
    };
  }, [absPath, kind]);
  return state;
}

// ─── Per-item actions (copy / reveal / save-as) ───────────────────────────────

async function copyImageToClipboard(absPath) {
  try {
    const res = await invoke('read_file_base64', { path: absPath });
    const ext = extOf(absPath);
    const mime = IMAGE_MIME[ext] || 'image/png';
    const blob = b64ToBlob(res.data, mime);
    // Chromium-based webviews accept PNG directly on the clipboard; for other
    // formats we transcode through a canvas so the clipboard always carries
    // image/png (the format other apps reliably read).
    if (mime === 'image/png') {
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
    } else {
      const dataUrl = `data:${mime};base64,${res.data}`;
      const png = await transcodeToPng(dataUrl);
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })]);
    }
    toast.success('Image copied');
  } catch (e) {
    toast.error(`Copy failed: ${e}`);
  }
}

function transcodeToPng(dataUrl) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      const ctx = c.getContext('2d');
      ctx.drawImage(img, 0, 0);
      c.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error('canvas toBlob failed'));
      }, 'image/png');
    };
    img.onerror = () => reject(new Error('image decode failed'));
    img.src = dataUrl;
  });
}

async function copyPathToClipboard(absPath) {
  try {
    await navigator.clipboard.writeText(absPath);
    toast.success('Path copied');
  } catch (e) {
    toast.error(`Copy failed: ${e}`);
  }
}

async function reveal(absPath) {
  try {
    await revealInFileManager(absPath);
  } catch (e) {
    toast.error(`Open failed: ${e}`);
  }
}

// ─── Thumbnail ────────────────────────────────────────────────────────────────

function MediaThumb({ absPath, kind, onClick }) {
  const { src, error, loading } = useMediaSrc(absPath, kind);

  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'group relative overflow-hidden rounded-md border border-border/60 bg-muted/30',
        'aspect-square w-full max-w-[180px] cursor-zoom-in transition-colors',
        'hover:border-border focus:outline-none focus-visible:ring-2 focus-visible:ring-ring',
      )}
      title={basenameOf(absPath)}
    >
      {loading && <Skeleton className="absolute inset-0" />}
      {error && !loading && (
        <div className="flex h-full w-full items-center justify-center p-2 text-center text-[10px] text-destructive">
          {error.length > 80 ? error.slice(0, 80) + '…' : error}
        </div>
      )}
      {src && kind === 'image' && (
        <img
          src={src}
          alt={basenameOf(absPath)}
          className="h-full w-full object-cover transition-transform group-hover:scale-[1.02]"
          draggable={false}
        />
      )}
      {src && kind === 'video' && (
        <>
          <video
            src={src}
            muted
            playsInline
            preload="metadata"
            className="h-full w-full object-cover"
          />
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-black/30 opacity-0 transition-opacity group-hover:opacity-100">
            <div className="rounded-full bg-white/90 p-2 text-black shadow">
              <svg viewBox="0 0 24 24" className="size-4 fill-current">
                <path d="M8 5v14l11-7z" />
              </svg>
            </div>
          </div>
        </>
      )}
    </button>
  );
}

// ─── Lightbox ─────────────────────────────────────────────────────────────────

function Lightbox({ items, index, onClose, onNavigate }) {
  const item = items[index];
  const { src, error, loading } = useMediaSrc(item?.absPath, item?.kind);

  // Per-item zoom + pan state. Reset whenever the visible item changes.
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  useEffect(() => {
    setScale(1);
    setOffset({ x: 0, y: 0 });
  }, [index]);

  // Keyboard navigation + close.
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') onClose();
      else if (e.key === 'ArrowLeft') onNavigate(-1);
      else if (e.key === 'ArrowRight') onNavigate(1);
      else if (e.key === '+' || e.key === '=') setScale((s) => Math.min(64, s * 1.25));
      else if (e.key === '-') setScale((s) => Math.max(0.1, s / 1.25));
      else if (e.key === '0') { setScale(1); setOffset({ x: 0, y: 0 }); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, onNavigate]);

  // Wheel zoom on the surface. Multi-touch / Ctrl+wheel feels natural; plain
  // wheel scrolls when zoomed in. We bind manually so we can preventDefault
  // (React's onWheel is passive by default).
  const surfaceRef = useRef(null);
  useEffect(() => {
    const el = surfaceRef.current;
    if (!el) return;
    const onWheel = (e) => {
      if (item?.kind !== 'image') return;
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      const factor = Math.exp(-e.deltaY / 400);
      setScale((s) => Math.min(64, Math.max(0.1, s * factor)));
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [item?.kind]);

  // Pan: only meaningful when zoomed past 1. Drag with primary button.
  const dragRef = useRef(null);
  const onPointerDown = (e) => {
    if (item?.kind !== 'image' || scale <= 1) return;
    e.preventDefault();
    dragRef.current = {
      startX: e.clientX,
      startY: e.clientY,
      baseX: offset.x,
      baseY: offset.y,
    };
    e.currentTarget.setPointerCapture?.(e.pointerId);
  };
  const onPointerMove = (e) => {
    if (!dragRef.current) return;
    setOffset({
      x: dragRef.current.baseX + (e.clientX - dragRef.current.startX),
      y: dragRef.current.baseY + (e.clientY - dragRef.current.startY),
    });
  };
  const onPointerUp = (e) => {
    dragRef.current = null;
    try { e.currentTarget.releasePointerCapture?.(e.pointerId); } catch {}
  };

  const isImage = item?.kind === 'image';

  return (
    <Dialog open={!!item} onOpenChange={(v) => !v && onClose()}>
      <DialogContent
        aria-describedby={undefined}
        className="!fixed !inset-2 !left-2 !top-2 !w-auto !max-w-none !translate-x-0 !translate-y-0 p-0 gap-0 bg-background/95 border-border/60 sm:!max-w-none"
        style={{ height: 'calc(100vh - 1rem)', width: 'calc(100vw - 1rem)' }}
      >
        <DialogTitle className="sr-only">
          {item?.kind === 'image' ? 'Image Viewer' : 'Video Viewer'}
        </DialogTitle>
        {/* Toolbar — extra right padding reserves space for DialogContent's
            built-in close button (absolute, top-right). Without it the last
            action button sits underneath the X. */}
        <div className="flex h-10 shrink-0 items-center gap-1 border-b border-border/60 pl-2 pr-10">
          <div className="min-w-0 flex-1 truncate text-[12px] text-muted-foreground">
            {basenameOf(item?.absPath || '')}
            {items.length > 1 && (
              <span className="ml-2 text-[11px] text-muted-foreground/60">
                {index + 1} / {items.length}
              </span>
            )}
          </div>
          {isImage && (
            <>
              <Button size="icon-xs" variant="ghost" onClick={() => setScale((s) => Math.max(0.1, s / 1.25))} title="Zoom out (-)">
                <ZoomOut className="size-3.5" />
              </Button>
              <span className="w-12 text-center text-[11px] text-muted-foreground">
                {Math.round(scale * 100)}%
              </span>
              <Button size="icon-xs" variant="ghost" onClick={() => setScale((s) => Math.min(64, s * 1.25))} title="Zoom in (+)">
                <ZoomIn className="size-3.5" />
              </Button>
              <Button size="icon-xs" variant="ghost" onClick={() => { setScale(1); setOffset({ x: 0, y: 0 }); }} title="Reset (0)">
                <Maximize2 className="size-3.5" />
              </Button>
              <div className="mx-1 h-5 w-px bg-border/60" />
              <Button
                size="icon-xs" variant="ghost"
                onClick={() => copyImageToClipboard(item.absPath)}
                title="Copy image"
              >
                <Copy className="size-3.5" />
              </Button>
            </>
          )}
          <Button
            size="icon-xs" variant="ghost"
            onClick={() => reveal(item.absPath)}
            title="Show in folder"
          >
            <FolderOpen className="size-3.5" />
          </Button>
          <Button
            size="icon-xs" variant="ghost"
            onClick={() => copyPathToClipboard(item.absPath)}
            title="Copy path"
          >
            <Download className="size-3.5" />
          </Button>
          {/* DialogContent's built-in close button (top-right) handles close;
              don't add our own X here or it shows up twice. Esc still works
              via the keyboard listener above. */}
        </div>

        {/* Surface */}
        <div
          ref={surfaceRef}
          className="relative flex flex-1 items-center justify-center overflow-hidden bg-black/40"
        >
          {loading && <Skeleton className="absolute inset-8" />}
          {error && !loading && (
            <div className="p-6 text-sm text-destructive">{error}</div>
          )}
          {src && isImage && (
            <img
              src={src}
              alt={basenameOf(item.absPath)}
              draggable={false}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={onPointerUp}
              style={{
                transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
                transformOrigin: 'center',
                cursor: scale > 1 ? (dragRef.current ? 'grabbing' : 'grab') : 'zoom-in',
                imageRendering: scale >= 4 ? 'pixelated' : 'auto',
                maxWidth: scale <= 1 ? '95%' : 'none',
                maxHeight: scale <= 1 ? '95%' : 'none',
              }}
              onDoubleClick={() => {
                if (scale === 1) setScale(2);
                else { setScale(1); setOffset({ x: 0, y: 0 }); }
              }}
            />
          )}
          {src && !isImage && (
            <video
              src={src}
              controls
              autoPlay
              playsInline
              className="max-h-full max-w-full bg-black"
            />
          )}

          {items.length > 1 && (
            <>
              <button
                type="button"
                onClick={() => onNavigate(-1)}
                className="absolute left-3 top-1/2 -translate-y-1/2 rounded-full bg-black/40 p-2 text-white hover:bg-black/60"
                aria-label="Previous"
              >
                <svg viewBox="0 0 24 24" className="size-4 fill-current"><path d="M15.5 19l-7-7 7-7v14z" /></svg>
              </button>
              <button
                type="button"
                onClick={() => onNavigate(1)}
                className="absolute right-3 top-1/2 -translate-y-1/2 rounded-full bg-black/40 p-2 text-white hover:bg-black/60"
                aria-label="Next"
              >
                <svg viewBox="0 0 24 24" className="size-4 fill-current"><path d="M8.5 5l7 7-7 7V5z" /></svg>
              </button>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ─── Public: MediaGallery ─────────────────────────────────────────────────────

/**
 * Renders the inline thumbnail strip + lightbox for a parsed media-output
 * payload. Designed to slot inside a tool-call card's output area, replacing
 * (or accompanying) the raw JSON dump.
 */
export function MediaGallery({ data }) {
  const projectRoot = useAgent((s) => s.activeProject.root);
  const [openIdx, setOpenIdx] = useState(null);

  const items = (data?.paths || []).map((rel) => {
    const absPath = joinAbs(projectRoot, rel);
    return { absPath, relPath: rel, kind: kindOf(rel) };
  });

  if (items.length === 0) return null;

  const onNavigate = (delta) => {
    setOpenIdx((cur) => {
      if (cur == null) return cur;
      const next = (cur + delta + items.length) % items.length;
      return next;
    });
  };

  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2 text-[10px] uppercase tracking-wide text-muted-foreground">
        <span>{items.length === 1 ? 'Generated' : `Generated · ${items.length}`}</span>
        {data?.provider && (
          <>
            <span className="text-muted-foreground/40">·</span>
            <span className="font-mono normal-case tracking-normal text-muted-foreground/80">
              {data.provider}{data.model ? ` / ${data.model}` : ''}
            </span>
          </>
        )}
        {typeof data?.cost_usd === 'number' && data.cost_usd > 0 && (
          <>
            <span className="text-muted-foreground/40">·</span>
            <span className="normal-case tracking-normal text-muted-foreground/80">
              ${data.cost_usd.toFixed(4)}
            </span>
          </>
        )}
      </div>

      <div className="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-2">
        {items.map((it, i) => (
          <div key={it.absPath + i} className="relative">
            <MediaThumb
              absPath={it.absPath}
              kind={it.kind}
              onClick={() => setOpenIdx(i)}
            />
            {/* Hover actions: per-item copy / reveal. */}
            <div className="pointer-events-none absolute right-1 top-1 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100 [.group:hover_&]:opacity-100">
              {it.kind === 'image' && (
                <Button
                  size="icon-xs"
                  variant="secondary"
                  className="pointer-events-auto size-6 bg-background/80 backdrop-blur"
                  onClick={(e) => { e.stopPropagation(); copyImageToClipboard(it.absPath); }}
                  title="Copy image"
                >
                  <Copy className="size-3" />
                </Button>
              )}
              <Button
                size="icon-xs"
                variant="secondary"
                className="pointer-events-auto size-6 bg-background/80 backdrop-blur"
                onClick={(e) => { e.stopPropagation(); reveal(it.absPath); }}
                title="Show in folder"
              >
                <FolderOpen className="size-3" />
              </Button>
            </div>
          </div>
        ))}
      </div>

      {openIdx != null && (
        <Lightbox
          items={items}
          index={openIdx}
          onClose={() => setOpenIdx(null)}
          onNavigate={onNavigate}
        />
      )}
    </div>
  );
}

export default MediaGallery;
