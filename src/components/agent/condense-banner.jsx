import React from 'react';
import { Loader2 } from 'lucide-react';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';

/**
 * CondenseBanner — renders an inline banner when the backend is compacting
 * the conversation context (condensing old messages into a summary).
 *
 * Visibility lifecycle:
 *   1. Context exceeds threshold (70% of available window)
 *   2. Backend emits `agent-context-condense-started`
 *   3. This banner appears
 *   4. Backend finishes condensing and emits `agent-context-condense-completed`
 *   5. Banner disappears automatically
 *
 * Unlike StreamRetryBanner, this doesn't show a countdown because condensing
 * is fast (typically 2-5 seconds) and the duration is unpredictable.
 */
export function CondenseBanner() {
  const condensing = useAgent((s) =>
    s.activeTaskId ? s.condensingByTask[s.activeTaskId] : null
  );

  if (!condensing) return null;

  return (
    <div
      className={cn(
        'mx-auto mb-2 w-full max-w-3xl px-3',
        // Match the prompt box's horizontal padding so the banner aligns
        // visually with the input below.
      )}
      role="status"
      aria-live="polite"
    >
      <div className="overflow-hidden rounded-md border border-blue-500/40 bg-blue-500/10 text-sm">
        <div className="flex items-start gap-3 px-3 py-2">
          <div className="mt-0.5 shrink-0 text-blue-600 dark:text-blue-400">
            <Loader2 className="size-4 animate-spin" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="font-medium text-blue-900 dark:text-blue-100">
              Compacting context…
            </div>
            <div className="mt-0.5 text-xs text-blue-800/90 dark:text-blue-200/80">
              Condensing older messages to preserve conversation continuity
            </div>
          </div>
        </div>
        {/* Indeterminate progress bar */}
        <div className="h-0.5 w-full overflow-hidden bg-blue-500/20">
          <div
            className="h-full w-1/3 bg-blue-500 animate-pulse"
          />
        </div>
      </div>
    </div>
  );
}
