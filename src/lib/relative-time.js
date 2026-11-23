import { useEffect, useState } from 'react';

/** Parse a backend timestamp (epoch ms, RFC 3339, or SQLite UTC) to epoch ms; 0 when absent or invalid. */
export function parseTimestamp(raw) {
  if (!raw) return 0;
  if (typeof raw === 'number') return Number.isFinite(raw) ? raw : 0;
  const s = String(raw).trim();
  // SQLite's datetime('now') is zone-less UTC (`YYYY-MM-DD HH:MM:SS`), which
  // `new Date()` reads as local time, skewing the label by the UTC offset.
  const zoneless = /^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(:\d{2}(\.\d+)?)?$/.test(s);
  const ms = Date.parse(zoneless ? `${s.replace(' ', 'T')}Z` : s);
  return Number.isFinite(ms) ? ms : 0;
}

// Compact "5s ago" / "2m ago" / "3h ago" / "2d ago" formatter for chat
// timestamps. Returns "just now" for anything under five seconds so freshly
// arriving tool calls don't flicker between "1s" and "2s" the moment they
// land. `null`/`undefined`/0 → empty string (caller decides to render nothing).
export function formatRelativeTime(ms, nowMs = Date.now()) {
  if (!ms) return '';
  const diff = Math.max(0, nowMs - ms);
  const secs = Math.floor(diff / 1000);
  if (secs < 5) return 'just now';
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  // Past a week the relative form stops being useful — fall back to a short
  // absolute date so the user can still tell roughly when something happened.
  try {
    return new Date(ms).toLocaleDateString(undefined, {
      month: 'short',
      day: 'numeric',
    });
  } catch {
    return `${days}d ago`;
  }
}

// React hook: returns the relative-time string for `ms` and triggers periodic
// re-renders so the label actually advances over time. Cadence is adaptive —
// fresh timestamps tick every 5s so the user sees "just now" → "10s ago" →
// "15s ago" smoothly; once we're past a minute we slow to 30s; past an hour,
// once a minute. Avoids waking 1× per second for cards that have been sitting
// in the transcript for hours.
export function useRelativeTime(ms) {
  const [, tick] = useState(0);
  useEffect(() => {
    if (!ms) return undefined;
    let cancelled = false;
    let timeoutId = null;
    const schedule = () => {
      if (cancelled) return;
      const age = Date.now() - ms;
      const next = age < 60_000 ? 5_000 : age < 3_600_000 ? 30_000 : 60_000;
      timeoutId = setTimeout(() => {
        tick((n) => n + 1);
        schedule();
      }, next);
    };
    schedule();
    return () => {
      cancelled = true;
      if (timeoutId) clearTimeout(timeoutId);
    };
  }, [ms]);
  return formatRelativeTime(ms);
}
