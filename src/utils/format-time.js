/**
 * Coarse "time ago" label shared by the agent panel and the welcome-screen
 * history list. Parses an ISO/RFC3339 string (or any Date-parseable value)
 * and renders a short relative form — "just now", "5m ago", "3h ago",
 * "2d ago", "4mo ago", "1y ago".
 *
 * Returns '' for missing or unparseable input so callers can cheaply skip
 * rendering the slot entirely.
 */
export function formatRelativeTime(value) {
  if (!value) return '';
  const then = new Date(value).getTime();
  if (!Number.isFinite(then) || then <= 0) return '';
  const diffSec = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (diffSec < 60) return 'just now';
  const m = Math.floor(diffSec / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d ago`;
  const mo = Math.floor(d / 30);
  if (mo < 12) return `${mo}mo ago`;
  const y = Math.floor(d / 365);
  return `${y}y ago`;
}
