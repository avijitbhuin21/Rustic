import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { basename } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';
import { PreviewSurface } from './preview-surface';
import { PanZoomControls, PanZoomSurface, usePanZoom } from './pan-zoom';

/** Preview for HEIC/HEIF, TIFF, camera RAW and ICNS — decoded by the backend. */
export default function DecodedImagePreview({ tab }) {
  const [result, setResult] = useState(null);
  const [error, setError] = useState(null);
  const reloadVersion = useFileReloadVersion(tab.path);
  const pz = usePanZoom({ fit: 'contain', maxScale: 64 });

  useEffect(() => {
    let cancelled = false;
    setResult(null);
    setError(null);
    invoke('decode_image_preview', { path: tab.path })
      .then((r) => { if (!cancelled) setResult(r); })
      .catch((e) => { if (!cancelled) setError(String(e?.message || e)); });
    return () => { cancelled = true; };
  }, [tab.path, reloadVersion]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6 text-center text-sm text-destructive">
        {error}
      </div>
    );
  }
  if (!result) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-2/3 w-2/3" />
      </div>
    );
  }

  const images = result.images || [];
  const single = images.length === 1;
  const first = images[0];
  const toolbar = (
    <>
      <div className="min-w-0 truncate text-xs text-muted-foreground">
        {basename(tab.path)}
        {single && first ? ` · ${first.width}×${first.height}` : ` · ${images.length} sizes`}
        {result.note ? ` · ${result.note}` : ''}
      </div>
      {single && <PanZoomControls pz={pz} />}
    </>
  );

  if (single) {
    return (
      <PanZoomSurface pz={pz} toolbar={toolbar}>
        <img
          src={`data:${first.mime};base64,${first.data}`}
          alt={basename(tab.path)}
          draggable={false}
          onLoad={() => pz.setContentSize(first.width, first.height)}
          style={{ display: 'block', width: first.width, height: first.height, maxWidth: 'none' }}
        />
      </PanZoomSurface>
    );
  }

  return (
    <PreviewSurface toolbar={toolbar}>
      {(
        <div className="flex flex-wrap items-end gap-6 p-6">
          {images.map((im) => (
            <figure key={im.label} className="flex flex-col items-center gap-2">
              <img
                src={`data:${im.mime};base64,${im.data}`}
                alt={im.label}
                style={{ width: Math.min(im.width, 256), height: 'auto' }}
                className="[image-rendering:auto]"
              />
              <figcaption className="text-[11px] text-muted-foreground">{im.label}</figcaption>
            </figure>
          ))}
        </div>
      )}
    </PreviewSurface>
  );
}
