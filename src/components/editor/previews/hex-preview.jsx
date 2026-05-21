import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';

const CHUNK_SIZE = 4096;

function formatOffset(n) {
  return n.toString(16).padStart(8, '0');
}

export default function HexPreview({ tab }) {
  const [chunk, setChunk] = useState(null);
  const [offset, setOffset] = useState(0);
  const [error, setError] = useState(null);

  const load = useCallback(
    (off) => {
      setChunk(null);
      invoke('read_hex_chunk', { path: tab.path, offset: off, length: CHUNK_SIZE })
        .then((res) => {
          setChunk(res);
          setOffset(off);
        })
        .catch((e) => setError(String(e)));
    },
    [tab.path]
  );

  useEffect(() => {
    load(0);
  }, [load]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }
  if (!chunk) {
    return (
      <div className="flex h-full w-full flex-col gap-1 p-4 font-mono text-xs">
        {Array.from({ length: 20 }).map((_, i) => (
          <Skeleton key={i} className="h-4 w-full" />
        ))}
      </div>
    );
  }

  const rows = [];
  for (let i = 0; i < chunk.hex.length; i += 16) {
    const hexCells = chunk.hex.slice(i, i + 16);
    const asciiCells = chunk.ascii.slice(i, i + 16);
    while (hexCells.length < 16) hexCells.push('  ');
    rows.push({
      addr: formatOffset(chunk.offset + i),
      hex: hexCells,
      ascii: asciiCells.join(''),
    });
  }

  const totalChunks = Math.ceil(chunk.total_size / CHUNK_SIZE) || 1;
  const currentChunk = Math.floor(offset / CHUNK_SIZE) + 1;
  const hasPrev = offset > 0;
  const hasNext = offset + chunk.bytes_read < chunk.total_size;

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <div className="flex h-9 shrink-0 items-center justify-between border-b border-border bg-muted/20 px-3 text-[11px] text-muted-foreground">
        <span>
          Offset 0x{formatOffset(offset)} of {chunk.total_size.toLocaleString()} bytes
        </span>
        <div className="flex items-center gap-1">
          <Button
            size="xs"
            variant="ghost"
            disabled={!hasPrev}
            onClick={() => load(Math.max(0, offset - CHUNK_SIZE))}
          >
            Prev
          </Button>
          <span>
            {currentChunk} / {totalChunks}
          </span>
          <Button
            size="xs"
            variant="ghost"
            disabled={!hasNext}
            onClick={() => load(offset + CHUNK_SIZE)}
          >
            Next
          </Button>
        </div>
      </div>
      <div className="flex-1 overflow-auto p-3 font-mono text-xs leading-5">
        {rows.map((r) => (
          <div key={r.addr} className="flex gap-4 whitespace-pre">
            <span className="text-muted-foreground">{r.addr}</span>
            <span className="text-foreground">{r.hex.join(' ')}</span>
            <span className="text-muted-foreground">{r.ascii}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
