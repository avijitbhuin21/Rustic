import React, { useEffect, useState } from 'react';
import { AlertTriangle, RefreshCw } from 'lucide-react';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';

/**
 * StreamRetryBanner — renders an inline "retrying in Xs" banner whenever the
 * backend has emitted a `agent-stream-retry` event for the active task and
 * the next attempt hasn't started yet.
 *
 * Visibility lifecycle (mirrors the backend retry loop):
 *   1. Provider call fails (rate limit / 5xx / stall)
 *   2. Backend emits `agent-stream-retry` with `waiting_ms` + `error`
 *   3. This banner appears and counts down to zero in 250 ms ticks
 *   4. Backend retries; on success the first stream chunk clears
 *      `retryByTask` in agent.js, which hides this banner
 *      automatically.
 *
 * The countdown is computed from `started_at_ms + waiting_ms`, not
 * decremented locally — so if the user backgrounds the app and comes back,
 * the timer reflects real wall-clock time, not whatever happened to be in
 * the React effect's closure.
 */
export function StreamRetryBanner() {
  const retry = useAgent((s) =>
    s.activeTaskId ? s.retryByTask[s.activeTaskId] : null
  );

  // Tick state purely so the countdown re-renders every 250 ms. The actual
  // remaining-time math derives from retry.started_at_ms + retry.waiting_ms
  // so we never drift from wall clock.
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!retry) return;
    const id = setInterval(() => setTick((t) => t + 1), 250);
    return () => clearInterval(id);
  }, [retry]);

  if (!retry) return null;

  const elapsed = Date.now() - retry.started_at_ms;
  const remaining = Math.max(0, retry.waiting_ms - elapsed);
  const secondsLeft = Math.ceil(remaining / 1000);
  const percent = retry.waiting_ms > 0
    ? Math.min(100, Math.max(0, (elapsed / retry.waiting_ms) * 100))
    : 100;

  // Once the timer expires, the backend is in the middle of issuing the
  // next request — show an "attempting…" message instead of "in 0s".
  const isAttempting = secondsLeft <= 0;

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
      <div className="overflow-hidden rounded-md border border-amber-500/40 bg-amber-500/10 text-sm">
        <div className="flex items-start gap-3 px-3 py-2">
          <div className="mt-0.5 shrink-0 text-amber-600 dark:text-amber-400">
            {isAttempting ? (
              <RefreshCw className="size-4 animate-spin" />
            ) : (
              <AlertTriangle className="size-4" />
            )}
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline justify-between gap-3">
              <div className="font-medium text-amber-900 dark:text-amber-100">
                {isAttempting
                  ? `Retrying… (attempt ${retry.attempt} of ${retry.max_attempts})`
                  : `Retrying in ${secondsLeft}s (attempt ${retry.attempt} of ${retry.max_attempts})`}
              </div>
            </div>
            {retry.error ? (
              <div className="mt-0.5 break-words text-xs text-amber-800/90 dark:text-amber-200/80">
                {retry.error}
              </div>
            ) : null}
          </div>
        </div>
        {/* Slim progress bar at the bottom of the banner — fills as the
            backoff elapses so the user has a visual sense of how close
            we are to the next attempt. */}
        <div className="h-0.5 w-full bg-amber-500/20">
          <div
            className="h-full bg-amber-500 transition-[width] duration-200 ease-linear"
            style={{ width: `${percent}%` }}
          />
        </div>
      </div>
    </div>
  );
}
