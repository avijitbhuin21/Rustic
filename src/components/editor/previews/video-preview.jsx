import React, { useEffect, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { basename } from '@/state/editor';
import { PreviewSurface } from './preview-surface';

/** Total byte size from a 1-byte Range probe (both stream endpoints send Content-Range). */
async function probeSize(url) {
  const res = await fetch(url, { headers: { Range: 'bytes=0-0' } });
  const total = res.headers.get('content-range')?.split('/')[1];
  if (!res.ok) throw new Error((await res.text().catch(() => '')) || `HTTP ${res.status}`);
  return total && total !== '*' ? Number(total) : null;
}

export default function VideoPreview({ tab }) {
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);

  // Stream from disk with HTTP Range requests (`rustic-media://` on desktop,
  // `/api/asset` on the web build) so clips of any size play without being
  // base64-encoded over IPC (capped at 100 MB) and buffered in memory.
  const reloadVersion = useFileReloadVersion(tab.path);
  const base = convertFileSrc(tab.path, 'rustic-media');
  const src = `${base}${base.includes('?') ? '&' : '?'}v=${reloadVersion}`;

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setSize(null);
    probeSize(src)
      .then((n) => { if (!cancelled) setSize(n); })
      // Size is cosmetic (desktop CSP may block fetch on the media scheme);
      // real load failures surface through the <video> onError below.
      .catch(() => {});
    return () => { cancelled = true; };
  }, [src]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
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
          onError={(e) => {
            const code = e.currentTarget.error?.code;
            setError(code === 4
              ? "This video format can't be played here."
              : 'Could not load the video.');
          }}
          // Avoid auto-play so dropping into a folder full of clips doesn't
          // turn into a chorus. Users hit play themselves.
          className="max-h-full max-w-full rounded-md bg-black shadow-lg"
        />
      </div>
    </PreviewSurface>
  );
}
