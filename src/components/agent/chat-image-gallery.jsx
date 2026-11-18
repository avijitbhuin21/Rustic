import React, { useEffect, useRef, useState } from 'react';
import {
  ChevronLeft,
  ChevronRight,
  Copy,
  ExternalLink,
  Image as ImageIcon,
  Loader2,
  MessageSquareText,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { fetchImageDataUrl } from '@/state/agent';
import { openFileMention } from '@/lib/file-mentions';
import { activeChatRoot } from '@/lib/file-links';
import { cn } from '@/lib/utils';

/** Converts a data URL into a PNG blob the OS clipboard reliably accepts. */
function dataUrlToPngBlob(dataUrl) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      c.getContext('2d').drawImage(img, 0, 0);
      c.toBlob((b) => (b ? resolve(b) : reject(new Error('encode failed'))), 'image/png');
    };
    img.onerror = () => reject(new Error('decode failed'));
    img.src = dataUrl;
  });
}

/** Copies an image (by its data/blob URL) to the clipboard as PNG. */
async function copyImage(src) {
  try {
    const blob = await dataUrlToPngBlob(src);
    await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
    toast.success('Image copied');
  } catch (e) {
    toast.error(`Copy failed: ${e?.message || e}`);
  }
}

/** Formats a message timestamp for the gallery source label. */
function formatWhen(ts) {
  if (!ts) return '';
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return '';
  const sameDay = d.toDateString() === new Date().toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    : d.toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** Resolves a gallery item's src, fetching deferred payloads on demand. */
function useItemSrc(item, known) {
  const direct = item ? item.att.url || item.att.src || known || null : null;
  const [fetched, setFetched] = useState(null);
  useEffect(() => {
    setFetched(null);
    if (!item || direct || !item.att.deferred) return undefined;
    let cancelled = false;
    const a = item.att;
    fetchImageDataUrl(a.taskId, a.sortOrder, a.blockIndex, a.mediaType).then((url) => {
      if (!cancelled && url) setFetched(url);
    });
    return () => {
      cancelled = true;
    };
  }, [item, direct]);
  return direct || fetched;
}

/** One thumbnail in the gallery strip; lazily resolves deferred images. */
function StripThumb({ item, known, active, onSelect, onResolved }) {
  const ref = useRef(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === 'undefined') {
      setVisible(true);
      return undefined;
    }
    const io = new IntersectionObserver(([e]) => {
      if (e.isIntersecting) {
        setVisible(true);
        io.disconnect();
      }
    });
    io.observe(el);
    return () => io.disconnect();
  }, []);
  const src = useItemSrc(visible ? item : null, known);
  useEffect(() => {
    if (src && !known) onResolved(item.ref, src);
  }, [src, known, item.ref, onResolved]);
  useEffect(() => {
    if (active) ref.current?.scrollIntoView({ block: 'nearest', inline: 'center', behavior: 'smooth' });
  }, [active]);
  return (
    <button
      ref={ref}
      type="button"
      onClick={onSelect}
      aria-label="Show image"
      aria-current={active ? 'true' : undefined}
      className={cn(
        'relative size-14 shrink-0 overflow-hidden rounded-md border bg-muted transition-opacity',
        active ? 'border-primary ring-2 ring-primary/40' : 'border-border/60 opacity-60 hover:opacity-100',
      )}
    >
      {src ? (
        <img src={src} alt="" className="size-full object-cover" draggable={false} />
      ) : (
        <span className="flex size-full items-center justify-center">
          <ImageIcon className="size-4 text-muted-foreground" />
        </span>
      )}
    </button>
  );
}

/** Full-screen carousel over every image in the chat, with source label + actions. */
export function ChatGalleryViewer({ items, index, srcs, onRemember, onSelect, onClose, onGoToMessage }) {
  const item = index >= 0 ? items[index] : null;
  const src = useItemSrc(item, item ? srcs.get(item.ref) : null);
  const open = !!item;
  const hasPrev = index > 0;
  const hasNext = index >= 0 && index < items.length - 1;

  useEffect(() => {
    if (src && item && !srcs.get(item.ref)) onRemember(item.ref, src);
  }, [src, item, srcs, onRemember]);

  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e) => {
      if (e.key === 'ArrowLeft' && hasPrev) {
        e.preventDefault();
        onSelect(index - 1);
      } else if (e.key === 'ArrowRight' && hasNext) {
        e.preventDefault();
        onSelect(index + 1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, index, hasPrev, hasNext, onSelect]);

  const navBtn =
    'absolute top-1/2 z-10 flex size-11 -translate-y-1/2 items-center justify-center rounded-full bg-background/70 text-foreground shadow-md backdrop-blur hover:bg-background disabled:pointer-events-none disabled:opacity-30';
  const actionBtn =
    'inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs text-foreground hover:bg-muted disabled:pointer-events-none disabled:opacity-40';
  const when = formatWhen(item?.timestamp);
  const who = item?.role === 'assistant' ? 'Agent' : 'You';

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="flex h-screen w-screen max-w-[100vw] flex-col gap-0 border-none bg-background/95 p-0 ring-0 shadow-none sm:max-w-[100vw]"
      >
        <DialogTitle className="sr-only">Chat images</DialogTitle>
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border/60 px-3">
          <div className="flex min-w-0 flex-1 items-center gap-2 text-xs">
            <span className="rounded-full border border-border/60 bg-muted/50 px-2 py-0.5 font-medium text-foreground">
              {who}
            </span>
            {when && <span className="tabular-nums text-muted-foreground">{when}</span>}
            {item?.alt && <span className="truncate text-muted-foreground">· {item.alt}</span>}
            <span className="ml-1 tabular-nums text-muted-foreground/70">
              {index + 1} / {items.length}
            </span>
          </div>
          <button type="button" className={actionBtn} onClick={() => onGoToMessage(item)}>
            <MessageSquareText className="size-3.5" />
            Go to this message
          </button>
          <button
            type="button"
            className={actionBtn}
            disabled={!item?.path}
            title={item?.path ? `Open ${item.path}` : 'This image has no file on disk'}
            onClick={() => {
              openFileMention(activeChatRoot(), item.path);
              onClose();
            }}
          >
            <ExternalLink className="size-3.5" />
            Open in editor
          </button>
          <button type="button" className={actionBtn} disabled={!src} onClick={() => copyImage(src)}>
            <Copy className="size-3.5" />
            Copy
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close gallery"
            className="ml-1 flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
          >
            <X className="size-4" />
          </button>
        </div>

        <div className="relative flex min-h-0 flex-1 items-center justify-center p-6" onClick={onClose}>
          {src ? (
            <img
              src={src}
              alt={item?.alt || 'attachment'}
              onClick={(e) => e.stopPropagation()}
              className="max-h-full max-w-full rounded-md object-contain shadow-2xl"
            />
          ) : (
            <Loader2 className="size-8 animate-spin text-muted-foreground" />
          )}
          {items.length > 1 && (
            <>
              <button
                type="button"
                disabled={!hasPrev}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(index - 1);
                }}
                aria-label="Previous image"
                className={cn(navBtn, 'left-4')}
              >
                <ChevronLeft className="size-6" />
              </button>
              <button
                type="button"
                disabled={!hasNext}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(index + 1);
                }}
                aria-label="Next image"
                className={cn(navBtn, 'right-4')}
              >
                <ChevronRight className="size-6" />
              </button>
            </>
          )}
        </div>

        {items.length > 1 && (
          <div className="flex shrink-0 justify-center border-t border-border/60 px-3 py-2">
            <div className="flex max-w-full gap-2 overflow-x-auto pb-1">
              {items.map((it, i) => (
                <StripThumb
                  key={i}
                  item={it}
                  known={srcs.get(it.ref)}
                  active={i === index}
                  onSelect={() => onSelect(i)}
                  onResolved={onRemember}
                />
              ))}
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
