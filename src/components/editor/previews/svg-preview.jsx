import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';

export default function SvgPreview({ tab }) {
  const [raw, setRaw] = useState(null);
  const [error, setError] = useState(null);
  const [mode, setMode] = useState('rendered');

  useEffect(() => {
    let cancelled = false;
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (!cancelled) setRaw(c ?? '');
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  const safe = useMemo(() => {
    if (!raw) return '';
    return DOMPurify.sanitize(raw, { USE_PROFILES: { svg: true, svgFilters: true } });
  }, [raw]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }
  if (raw == null) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-64 w-64" />
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-muted/20 px-2">
        <Button
          size="xs"
          variant={mode === 'rendered' ? 'secondary' : 'ghost'}
          onClick={() => setMode('rendered')}
        >
          Rendered
        </Button>
        <Button
          size="xs"
          variant={mode === 'source' ? 'secondary' : 'ghost'}
          onClick={() => setMode('source')}
        >
          Source
        </Button>
      </div>
      {mode === 'rendered' ? (
        <div className="flex flex-1 items-center justify-center overflow-auto bg-black/30 p-4">
          <div
            className="[&_svg]:max-h-full [&_svg]:max-w-full"
            dangerouslySetInnerHTML={{ __html: safe }}
          />
        </div>
      ) : (
        <pre className="flex-1 overflow-auto bg-background p-4 text-xs text-foreground">
          {raw}
        </pre>
      )}
    </div>
  );
}
