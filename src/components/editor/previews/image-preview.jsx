import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Skeleton } from '@/components/ui/skeleton';
import { basename } from '@/state/editor';

const MIME = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  avif: 'image/avif',
};

function mimeFor(path) {
  const dot = path.lastIndexOf('.');
  const ext = dot < 0 ? '' : path.slice(dot + 1).toLowerCase();
  return MIME[ext] ?? 'application/octet-stream';
}

export default function ImagePreview({ tab }) {
  const [src, setSrc] = useState(null);
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);

  useEffect(() => {
    let cancelled = false;
    invoke('read_file_base64', { path: tab.path })
      .then((res) => {
        if (cancelled) return;
        setSrc(`data:${mimeFor(tab.path)};base64,${res.data}`);
        setSize(res.size);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (!src) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-64 w-64" />
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex-1 overflow-auto bg-black/30 p-4">
        <div className="flex h-full w-full items-center justify-center">
          <img
            src={src}
            alt={basename(tab.path)}
            className="max-h-full max-w-full object-contain"
            style={{ imageRendering: 'pixelated' }}
          />
        </div>
      </div>
      <div className="flex h-6 shrink-0 items-center justify-between border-t border-border bg-muted/30 px-3 text-[11px] text-muted-foreground">
        <span>{basename(tab.path)}</span>
        {size != null && <span>{(size / 1024).toFixed(1)} KB</span>}
      </div>
    </div>
  );
}
