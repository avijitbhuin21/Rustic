import React, { useState } from 'react';
import { OctagonAlert, Wrench } from 'lucide-react';
import { useAgent } from '@/state/agent';

/**
 * ProviderErrorBanner — offers a one-click "Repair & continue" when a turn
 * failed with a deterministic provider 4xx (a request the provider will
 * always reject, e.g. a mislabeled image block poisoning the history).
 *
 * Lifecycle:
 *   1. Backend emits `agent-provider-error` after a non-retryable 4xx
 *   2. The store repairs + resumes automatically, up to three times per
 *      distinct error signature
 *   3. Once that budget is spent (or nothing was repairable) this banner
 *      appears with the error text and a manual repair button
 *   4. The banner clears on the next send (or a successful repair)
 */
export function ProviderErrorBanner() {
  const taskId = useAgent((s) => s.activeTaskId);
  const entry = useAgent((s) => (s.activeTaskId ? s.providerErrorByTask[s.activeTaskId] : null));
  const repairAndContinue = useAgent((s) => s.repairAndContinue);
  const streaming = useAgent((s) => (s.activeTaskId ? s.streamingByTask[s.activeTaskId] : false));
  const [busy, setBusy] = useState(false);

  if (!entry || streaming) return null;

  const onRepair = async () => {
    setBusy(true);
    try {
      await repairAndContinue(taskId);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto mb-2 w-full max-w-3xl px-3" role="alert">
      <div className="overflow-hidden rounded-md border border-red-500/40 bg-red-500/10 text-sm">
        <div className="flex items-start gap-3 px-3 py-2">
          <div className="mt-0.5 shrink-0 text-red-600 dark:text-red-400">
            <OctagonAlert className="size-4" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="font-medium text-red-900 dark:text-red-100">
              {entry.autoExhausted
                ? `Auto-repair gave up after ${entry.attempts || 0} attempt(s) — this needs you.`
                : "The provider rejected the request — retrying won't help."}
            </div>
            <div className="mt-0.5 break-words text-xs text-red-800/90 dark:text-red-200/80">
              {entry.error}
            </div>
            <div className="mt-2 flex items-center gap-2">
              <button
                type="button"
                onClick={onRepair}
                disabled={busy}
                className="inline-flex items-center gap-1.5 rounded-md border border-red-500/50 bg-red-500/15 px-2.5 py-1 text-xs font-medium text-red-900 hover:bg-red-500/25 disabled:opacity-50 dark:text-red-100"
              >
                <Wrench className="size-3.5" />
                {busy ? 'Repairing…' : 'Repair & continue'}
              </button>
              <span className="text-[11px] text-red-800/70 dark:text-red-200/60">
                {entry.autoExhausted
                  ? 'Automatic attempts are used up — repair once more by hand, or edit and re-send.'
                  : 'Converts the rejected content in history to text, then resumes.'}
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
