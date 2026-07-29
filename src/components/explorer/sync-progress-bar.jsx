import React, { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { CloudDownload, CloudUpload, Check } from 'lucide-react';
import { cn } from '@/lib/utils';

const PHASE_LABELS = {
  connecting: 'Connecting',
  preparing: 'Preparing',
  archiving: 'Packing files',
  compressing: 'Compressing',
  uploading: 'Uploading',
  applying: 'Server applying',
  packing: 'Server packing',
  downloading: 'Downloading',
  extracting: 'Extracting',
  installing: 'Installing',
  writing: 'Writing files',
  finalizing: 'Finalizing',
  done: 'Done',
};

/// Compact cloud-sync progress strip shown in the explorer while a push or
/// pull runs (whole-environment or single project), driven by the backend's
/// `rustic:sync-progress` events.
export function SyncProgressBar() {
  const [progress, setProgress] = useState(null);

  useEffect(() => {
    let unlisten = null;
    let timer = null;
    listen('rustic:sync-progress', (e) => {
      const payload = e.payload ?? {};
      setProgress(payload);
      if (timer) clearTimeout(timer);
      if (payload.phase === 'done') {
        timer = setTimeout(() => setProgress(null), 3000);
      }
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      if (unlisten) unlisten();
      if (timer) clearTimeout(timer);
    };
  }, []);

  if (!progress) return null;

  const total = Number(progress.total) || 0;
  const done = Number(progress.done) || 0;
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : null;
  const finished = progress.phase === 'done';
  const Icon = finished ? Check : progress.direction === 'pull' ? CloudDownload : CloudUpload;

  return (
    <div className="flex flex-col gap-1 border-b border-border/60 bg-muted/40 px-2 py-1.5">
      <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
        <Icon className={cn('size-3 shrink-0', finished ? 'text-primary' : 'text-foreground/70')} />
        <span className="font-medium text-foreground/90">
          {PHASE_LABELS[progress.phase] ?? progress.phase}
        </span>
        {pct !== null && <span className="tabular-nums">{pct}%</span>}
        <span className="min-w-0 flex-1 truncate text-right">{progress.detail}</span>
      </div>
      <div className="h-1 w-full overflow-hidden rounded-full bg-border">
        <div
          className={cn(
            'h-full rounded-full bg-primary transition-[width] duration-200',
            pct === null && 'w-1/3 animate-pulse'
          )}
          style={pct === null ? undefined : { width: `${pct}%` }}
        />
      </div>
    </div>
  );
}
