import React, { useEffect, useState } from 'react';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { fetchFileBytes } from '@/lib/file-bytes';
import { basename } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';
import { Input } from '@/components/ui/input';
import { PreviewSurface } from './preview-surface';

const SIZES = [12, 16, 24, 36, 48, 72];
const GLYPHS =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()-_=+[]{};:\'",.<>/?\\|`~';

let fontSeq = 0;

/** Preview for TTF/OTF/WOFF/WOFF2: loads the font via the FontFace API and shows samples. */
export default function FontPreview({ tab }) {
  const [family, setFamily] = useState(null);
  const [error, setError] = useState(null);
  const [sample, setSample] = useState('The quick brown fox jumps over the lazy dog');
  const reloadVersion = useFileReloadVersion(tab.path);

  useEffect(() => {
    let cancelled = false;
    let face = null;
    setFamily(null);
    setError(null);
    const name = `rustic-font-preview-${++fontSeq}`;
    fetchFileBytes(tab.path, { version: reloadVersion })
      .then(async (bytes) => {
        face = new FontFace(name, bytes.buffer);
        await face.load();
        if (cancelled) return;
        document.fonts.add(face);
        setFamily(name);
      })
      .catch((e) => {
        if (!cancelled) setError(`Could not load this font (${String(e?.message || e)}).`);
      });
    return () => {
      cancelled = true;
      if (face) document.fonts.delete(face);
    };
  }, [tab.path, reloadVersion]);

  if (error) {
    return <div className="flex h-full items-center justify-center p-6 text-sm text-destructive">{error}</div>;
  }
  if (!family) {
    return <div className="p-6"><Skeleton className="h-40 w-full" /></div>;
  }

  const toolbar = (
    <>
      <div className="truncate text-xs text-muted-foreground">{basename(tab.path)}</div>
      <Input
        value={sample}
        onChange={(e) => setSample(e.target.value)}
        className="h-7 max-w-sm text-xs"
        placeholder="Sample text"
      />
    </>
  );

  return (
    <PreviewSurface toolbar={toolbar}>
      <div className="min-h-full bg-background p-6 text-foreground" style={{ fontFamily: `"${family}"` }}>
        <div className="space-y-4">
          {SIZES.map((s) => (
            <div key={s} className="flex items-baseline gap-4">
              <span className="w-10 shrink-0 font-sans text-[11px] text-muted-foreground">{s}px</span>
              <span style={{ fontSize: s, lineHeight: 1.25 }} className="break-words">{sample}</span>
            </div>
          ))}
        </div>
        <div className="mt-8 grid grid-cols-[repeat(auto-fill,minmax(3rem,1fr))] gap-1">
          {Array.from(GLYPHS).map((g, i) => (
            <div
              key={i}
              className="flex aspect-square items-center justify-center rounded-md border border-border text-2xl"
            >
              {g}
            </div>
          ))}
        </div>
      </div>
    </PreviewSurface>
  );
}
