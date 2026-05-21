import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { ChevronLeft, ChevronRight, ZoomIn, ZoomOut } from 'lucide-react';

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

function base64ToBytes(b64) {
  const binary = atob(b64);
  const len = binary.length;
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export default function PdfPreview({ tab }) {
  const canvasRef = useRef(null);
  const docRef = useRef(null);
  const renderTaskRef = useRef(null);
  const [page, setPage] = useState(1);
  const [pageCount, setPageCount] = useState(0);
  const [scale, setScale] = useState(1.25);
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    (async () => {
      try {
        const [pdfjs, b64] = await Promise.all([
          loadPdfJs(),
          invoke('read_file_base64', { path: tab.path }),
        ]);
        if (cancelled) return;
        const bytes = base64ToBytes(b64.data);
        const task = pdfjs.getDocument({ data: bytes });
        const doc = await task.promise;
        if (cancelled) {
          doc.destroy();
          return;
        }
        docRef.current = doc;
        setPageCount(doc.numPages);
        setPage(1);
        setLoading(false);
      } catch (e) {
        if (!cancelled) {
          setError(String(e));
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
      if (renderTaskRef.current) {
        try { renderTaskRef.current.cancel(); } catch {}
      }
      if (docRef.current) {
        try { docRef.current.destroy(); } catch {}
        docRef.current = null;
      }
    };
  }, [tab.path]);

  useEffect(() => {
    const doc = docRef.current;
    const canvas = canvasRef.current;
    if (!doc || !canvas || pageCount === 0) return;
    let cancelled = false;
    (async () => {
      try {
        const pdfPage = await doc.getPage(page);
        if (cancelled) return;
        const viewport = pdfPage.getViewport({ scale });
        const ctx = canvas.getContext('2d');
        canvas.width = viewport.width;
        canvas.height = viewport.height;
        if (renderTaskRef.current) {
          try { renderTaskRef.current.cancel(); } catch {}
        }
        const task = pdfPage.render({ canvasContext: ctx, viewport });
        renderTaskRef.current = task;
        await task.promise;
      } catch (e) {
        if (e?.name !== 'RenderingCancelledException') {
          setError(String(e));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [page, scale, pageCount]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center justify-between border-b border-border bg-muted/20 px-2">
        <div className="flex items-center gap-1">
          <Button
            size="icon-xs"
            variant="ghost"
            disabled={page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            <ChevronLeft />
          </Button>
          <span className="text-xs text-muted-foreground">
            {page} / {pageCount || '?'}
          </span>
          <Button
            size="icon-xs"
            variant="ghost"
            disabled={page >= pageCount}
            onClick={() => setPage((p) => Math.min(pageCount, p + 1))}
          >
            <ChevronRight />
          </Button>
        </div>
        <div className="flex items-center gap-1">
          <Button size="icon-xs" variant="ghost" onClick={() => setScale((s) => Math.max(0.5, s - 0.25))}>
            <ZoomOut />
          </Button>
          <span className="text-xs text-muted-foreground">{Math.round(scale * 100)}%</span>
          <Button size="icon-xs" variant="ghost" onClick={() => setScale((s) => Math.min(4, s + 0.25))}>
            <ZoomIn />
          </Button>
        </div>
      </div>
      <div className="flex flex-1 items-start justify-center overflow-auto bg-black/30 p-4">
        {loading ? (
          <Skeleton className="h-[800px] w-[600px]" />
        ) : (
          <canvas ref={canvasRef} className="shadow-lg" />
        )}
      </div>
    </div>
  );
}
