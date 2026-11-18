// Live MCP server status. The backend connects servers on background threads
// and pushes `mcp-server-status` ({ id, status, seq }) as each one settles, so
// lists render every configured server at once and fill in the result per row.
//
// A push and a list response travel separately and can arrive in either order.
// Both carry the backend's status sequence, and the higher one always wins, so
// a slow list response can never put a settled row back to "connecting".
import { useEffect, useRef } from 'react';
import { isTauriAvailable } from '@/lib/platform';

const latest = new Map(); // server id -> { status, seq } of the newest push seen
const subscribers = new Set();
let listening = null;

/** Start the single app-wide status listener; resolves once pushes are being received. */
export function mcpStatusReady() {
  if (listening) return listening;
  if (!isTauriAvailable()) {
    listening = Promise.resolve();
    return listening;
  }
  listening = import('@tauri-apps/api/event')
    .then(({ listen }) => listen('mcp-server-status', (e) => {
      const evt = e?.payload;
      if (!evt?.id || typeof evt.seq !== 'number') return;
      const known = latest.get(evt.id);
      if (known && known.seq >= evt.seq) return;
      latest.set(evt.id, { status: evt.status, seq: evt.seq });
      subscribers.forEach((fn) => fn(evt));
    }))
    .then(() => {}, () => {});
  return listening;
}

/** Call `onStatus({ id, status, seq })` whenever a server's connection status changes. */
export function useMcpServerStatus(onStatus) {
  const ref = useRef(onStatus);
  ref.current = onStatus;
  useEffect(() => {
    const fn = (evt) => ref.current?.(evt);
    subscribers.add(fn);
    mcpStatusReady();
    return () => { subscribers.delete(fn); };
  }, []);
}

/** Copy of `servers` with the row matching a status push updated, unless the row is already newer. */
export function applyMcpStatus(servers, { id, status, seq }) {
  if (!Array.isArray(servers)) return servers;
  const stale = (s) => s.id === id && (s.status_seq ?? -1) < seq;
  if (!servers.some(stale)) return servers;
  return servers.map((s) => (stale(s) ? { ...s, status, status_seq: seq } : s));
}

/** A freshly fetched server list with any push newer than its snapshot applied on top. */
export function reconcileMcpList(list) {
  return list.map((row) => {
    const known = latest.get(row.id);
    return known && known.seq > (row.status_seq ?? -1)
      ? { ...row, status: known.status, status_seq: known.seq }
      : row;
  });
}
