import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { File, Folder } from 'lucide-react';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { basename } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';

const IMG_MIME = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  webp: 'image/webp', bmp: 'image/bmp', svg: 'image/svg+xml', ico: 'image/x-icon', avif: 'image/avif',
};

function formatBytes(n) {
  if (n == null) return '';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

function b64ToBytes(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Text if the bytes look like UTF-8 without NULs, otherwise null. */
function asText(bytes) {
  const probe = bytes.subarray(0, 8192);
  if (probe.includes(0)) return null;
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

function hexDump(bytes, limit = 4096) {
  const lines = [];
  for (let off = 0; off < Math.min(bytes.length, limit); off += 16) {
    const row = bytes.subarray(off, off + 16);
    const hex = Array.from(row, (b) => b.toString(16).padStart(2, '0')).join(' ');
    const ascii = Array.from(row, (b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : '.')).join('');
    lines.push(`${off.toString(16).padStart(8, '0')}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return lines.join('\n');
}

/** Member viewer: text, image, or hex. */
function EntryView({ path, entry }) {
  const [state, setState] = useState({ loading: true });
  useEffect(() => {
    let cancelled = false;
    setState({ loading: true });
    invoke('archive_read_entry', { path, entry: entry.name })
      .then((r) => {
        if (cancelled) return;
        const bytes = b64ToBytes(r.data);
        const ext = entry.name.toLowerCase().split('.').pop();
        if (IMG_MIME[ext]) setState({ image: `data:${IMG_MIME[ext]};base64,${r.data}` });
        else {
          const text = asText(bytes);
          setState(text != null ? { text } : { hex: hexDump(bytes), size: bytes.length });
        }
      })
      .catch((e) => { if (!cancelled) setState({ error: String(e?.message || e) }); });
    return () => { cancelled = true; };
  }, [path, entry.name]);

  if (state.loading) return <div className="p-4"><Skeleton className="h-32 w-full" /></div>;
  if (state.error) return <div className="p-4 text-sm text-destructive">{state.error}</div>;
  if (state.image) {
    return (
      <div className="preview-checkerboard flex h-full items-center justify-center p-4">
        <img src={state.image} alt={entry.name} className="max-h-full max-w-full object-contain" />
      </div>
    );
  }
  if (state.text != null) {
    return <pre className="whitespace-pre-wrap break-words p-3 font-mono text-[12px] leading-relaxed">{state.text}</pre>;
  }
  return (
    <div className="p-3">
      <div className="mb-2 text-[11px] text-muted-foreground">
        Binary · {formatBytes(state.size)}{state.size > 4096 ? ' · first 4 KB shown' : ''}
      </div>
      <pre className="font-mono text-[11px] leading-relaxed">{state.hex}</pre>
    </div>
  );
}

/** Preview for zip-family, tar, tar.gz and gz archives. */
export default function ArchivePreview({ tab }) {
  const [listing, setListing] = useState(null);
  const [error, setError] = useState(null);
  const [filter, setFilter] = useState('');
  const [selected, setSelected] = useState(null);
  const reloadVersion = useFileReloadVersion(tab.path);

  useEffect(() => {
    let cancelled = false;
    setListing(null);
    setError(null);
    setSelected(null);
    invoke('archive_list', { path: tab.path })
      .then((r) => { if (!cancelled) setListing(r); })
      .catch((e) => { if (!cancelled) setError(String(e?.message || e)); });
    return () => { cancelled = true; };
  }, [tab.path, reloadVersion]);

  const entries = useMemo(() => {
    const all = listing?.entries || [];
    const q = filter.trim().toLowerCase();
    const shown = q ? all.filter((e) => e.name.toLowerCase().includes(q)) : all;
    return [...shown].sort((a, b) => a.name.localeCompare(b.name));
  }, [listing, filter]);

  if (error) return <div className="flex h-full items-center justify-center p-6 text-sm text-destructive">{error}</div>;
  if (!listing) return <div className="p-6"><Skeleton className="h-48 w-full" /></div>;

  const files = listing.entries.filter((e) => !e.is_dir);
  const total = files.reduce((s, e) => s + (e.size || 0), 0);

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-2">
        <div className="min-w-0 truncate text-xs text-muted-foreground">
          {basename(tab.path)} · {listing.kind} · {files.length} files · {formatBytes(total)} uncompressed
        </div>
        <Input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter…"
          className="ml-auto h-7 max-w-[14rem] text-xs"
        />
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="w-2/5 min-w-[14rem] overflow-auto border-r border-border">
          {entries.map((e) => (
            <button
              key={e.name}
              type="button"
              disabled={e.is_dir}
              onClick={() => setSelected(e)}
              className={cn(
                'flex w-full items-center gap-2 px-2 py-1 text-left text-[12px] hover:bg-muted/60 disabled:cursor-default',
                selected?.name === e.name && 'bg-muted',
              )}
            >
              {e.is_dir
                ? <Folder className="size-3.5 shrink-0 text-muted-foreground" />
                : <File className="size-3.5 shrink-0 text-muted-foreground" />}
              <span className="min-w-0 flex-1 truncate font-mono">{e.name}</span>
              {!e.is_dir && <span className="shrink-0 text-[11px] text-muted-foreground">{formatBytes(e.size)}</span>}
            </button>
          ))}
          {entries.length === 0 && <div className="p-3 text-xs text-muted-foreground">No matching entries.</div>}
        </div>
        <div className="min-w-0 flex-1 overflow-auto">
          {selected
            ? <EntryView key={selected.name} path={tab.path} entry={selected} />
            : <div className="flex h-full items-center justify-center text-xs text-muted-foreground">Select a file to view it.</div>}
        </div>
      </div>
    </div>
  );
}
