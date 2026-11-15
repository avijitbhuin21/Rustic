import React, { useEffect, useState } from 'react';
import { fileSize, fileStreamUrl } from '@/lib/file-bytes';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { Skeleton } from '@/components/ui/skeleton';
import { basename } from '@/state/editor';
import { PanZoomControls, PanZoomSurface, usePanZoom } from './pan-zoom';

/** Image preview: streamed from disk, wheel zooms around the cursor, drag pans. */
export default function ImagePreview({ tab }) {
  const [src, setSrc] = useState(null);
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);
  const [dims, setDims] = useState(null);
  const reloadVersion = useFileReloadVersion(tab.path);
  const pz = usePanZoom({ fit: 'contain', maxScale: 64, keepLayer: true });

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setDims(null);
    setSize(null);
    setSrc(fileStreamUrl(tab.path, reloadVersion));
    fileSize(tab.path, reloadVersion)
      .then((n) => { if (!cancelled) setSize(n); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [tab.path, reloadVersion]);

  if (error) {
    return <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">{error}</div>;
  }

  // Pixel art (icons, sprites) stays crisp when magnified; photos stay smooth.
  const pixelated = dims && (dims.w <= 256 || dims.h <= 256) && pz.scale >= 2;

  const toolbar = (
    <>
      <div className="truncate text-xs text-muted-foreground">
        {basename(tab.path)}
        {dims && <span className="ml-2 text-muted-foreground/60">{dims.w} × {dims.h}</span>}
      </div>
      <div className="flex items-center gap-2">
        <PanZoomControls pz={pz} />
        {size != null && (
          <span className="text-[11px] text-muted-foreground">
            {size >= 1048576 ? `${(size / 1048576).toFixed(1)} MB` : `${(size / 1024).toFixed(1)} KB`}
          </span>
        )}
      </div>
    </>
  );

  return (
    <PanZoomSurface pz={pz} toolbar={toolbar}>
      {!dims && <Skeleton className="h-64 w-64" />}
      {src && (
        <img
          src={src}
          alt={basename(tab.path)}
          draggable={false}
          onError={() => setError('This image could not be decoded.')}
          onLoad={(e) => {
            const { naturalWidth: w, naturalHeight: h } = e.currentTarget;
            setDims({ w, h });
            pz.setContentSize(w, h);
          }}
          style={{
            display: dims ? 'block' : 'none',
            width: dims?.w,
            height: dims?.h,
            maxWidth: 'none',
            imageRendering: pixelated ? 'pixelated' : 'auto',
          }}
        />
      )}
    </PanZoomSurface>
  );
}
