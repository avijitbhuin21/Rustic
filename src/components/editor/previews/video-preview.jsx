import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Skeleton } from '@/components/ui/skeleton';
import { basename } from '@/state/editor';
import { PreviewSurface } from './preview-surface';

const MIME = {
  mp4: 'video/mp4',
  m4v: 'video/mp4',
  webm: 'video/webm',
  mov: 'video/quicktime',
  mkv: 'video/x-matroska',
  ogv: 'video/ogg',
  avi: 'video/x-msvideo',
};

function mimeFor(path) {
  const dot = path.lastIndexOf('.');
  const ext = dot < 0 ? '' : path.slice(dot + 1).toLowerCase();
  return MIME[ext] ?? 'video/mp4';
}

function base64ToBlob(b64, type) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new Blob([bytes], { type });
}

export default function VideoPreview({ tab }) {
  const [src, setSrc] = useState(null);
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);

  // Decode the file into a Blob and hand the <video> element an object URL.
  // We deliberately avoid a `data:` src — Chromium-based webviews can decode
  // those but performance is poor for any clip more than a few seconds long
  // because the whole base64 string is re-parsed on every seek. Blob URLs
  // give the player a real byte range to work with.
  useEffect(() => {
    let cancelled = false;
    let url = null;
    setError(null);
    setSrc(null);
    setSize(null);
    invoke('read_file_base64', { path: tab.path })
      .then((res) => {
        if (cancelled) return;
        const blob = base64ToBlob(res.data, mimeFor(tab.path));
        url = URL.createObjectURL(blob);
        setSrc(url);
        setSize(res.size);
      })
      .catch((e) => {
        if (!cancelled) {
          // Tauri's preview command caps at 100MB; surface a friendly hint
          // when a clip exceeds that rather than dumping the raw error.
          const msg = String(e || '');
          if (msg.includes('too large')) {
            setError('Video is larger than 100MB — preview unavailable.');
          } else {
            setError(msg);
          }
        }
      });
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
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
        <Skeleton className="h-48 w-80" />
      </div>
    );
  }

  const toolbar = (
    <>
      <div className="truncate text-xs text-muted-foreground">
        {basename(tab.path)}
      </div>
      {size != null && (
        <span className="text-[11px] text-muted-foreground">
          {(size / (1024 * 1024)).toFixed(1)} MB
        </span>
      )}
    </>
  );

  // Video doesn't participate in pinch-zoom semantics — the user resizes by
  // dragging the pane, not by Ctrl+wheel — so we omit onScaleChange and let
  // Ctrl+wheel reach the host's default behaviour (which is a no-op since
  // the surface itself doesn't bind it without that callback).
  return (
    <PreviewSurface toolbar={toolbar}>
      <div className="flex h-full min-h-full w-full items-center justify-center p-4">
        <video
          src={src}
          controls
          playsInline
          // Avoid auto-play so dropping into a folder full of clips doesn't
          // turn into a chorus. Users hit play themselves.
          className="max-h-full max-w-full rounded-md bg-black shadow-lg"
        />
      </div>
    </PreviewSurface>
  );
}
