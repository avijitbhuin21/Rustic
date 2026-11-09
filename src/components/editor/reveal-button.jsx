import React from 'react';
import { FolderOpen } from 'lucide-react';
import { toast } from 'sonner';
import { revealInFileManager } from '@/state/explorer';
import { IS_MAC_DESKTOP } from '@/lib/platform';
import { cn } from '@/lib/utils';

const IS_WINDOWS = typeof navigator !== 'undefined' && /win/i.test(navigator.platform || navigator.userAgent || '');

/** OS-appropriate label for the reveal action ("Reveal in Finder" etc.). */
export function revealLabel() {
  if (IS_MAC_DESKTOP) return 'Reveal in Finder';
  if (IS_WINDOWS) return 'Reveal in File Explorer';
  return 'Reveal in File Manager';
}

/** Opens the OS file manager with `path` selected, toasting on failure. */
export async function revealPath(path) {
  try {
    await revealInFileManager(path);
  } catch (err) {
    toast.error(`Could not reveal: ${err}`);
  }
}

/** Small button that reveals `path` in Finder / File Explorer. */
export function RevealButton({ path, showLabel = false, className }) {
  if (!path) return null;
  const label = revealLabel();
  return (
    <button
      type="button"
      onClick={() => revealPath(path)}
      title={label}
      aria-label={label}
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded-sm px-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
        className,
      )}
    >
      <FolderOpen className="size-3.5" />
      {showLabel && <span>{label}</span>}
    </button>
  );
}
