import React from 'react';
import { FileQuestion } from 'lucide-react';
import { IS_WEB } from '@/lib/platform';
import { RevealButton } from './reveal-button';

/** Placeholder shown for files Rustic can't preview or edit yet. */
export function UnsupportedFileView({ tab }) {
  const name = (tab?.path ?? '').replace(/\\/g, '/').split('/').pop() || tab?.title || 'This file';
  const dot = name.lastIndexOf('.');
  const ext = dot > 0 ? name.slice(dot + 1).toUpperCase() : null;
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-6 text-center">
      <FileQuestion className="size-10 text-muted-foreground/60" />
      <div className="text-sm font-medium text-foreground">File type not supported yet</div>
      <div className="max-w-sm text-xs text-muted-foreground">
        {ext ? `${ext} files` : `"${name}"`} can't be previewed or edited in Rustic.
        {!IS_WEB && ' Open it with another app from your file manager.'}
      </div>
      {!IS_WEB && tab?.path && (
        <RevealButton
          path={tab.path}
          showLabel
          className="mt-1 h-7 rounded-md border border-border px-2.5 text-xs"
        />
      )}
    </div>
  );
}
