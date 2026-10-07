// Formatting helpers for transfer sizes, speeds and ETAs.

/** Human-readable byte count, e.g. "832.0 MB". */
export function formatBytes(n) {
  const v = Number(n) || 0;
  if (v < 1024) return `${v} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let x = v / 1024;
  let i = 0;
  while (x >= 1024 && i < units.length - 1) { x /= 1024; i += 1; }
  return `${x.toFixed(x >= 100 ? 0 : 1)} ${units[i]}`;
}

/** Transfer speed, e.g. "12.4 MB/s". */
export function formatSpeed(bps) {
  return bps > 0 ? `${formatBytes(Math.round(bps))}/s` : '';
}

/** Remaining time, e.g. "2m 05s". */
export function formatEta(secs) {
  if (secs == null || !Number.isFinite(secs)) return '';
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`;
}
