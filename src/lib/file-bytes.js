// Shared file access for previews. Everything streams from disk over HTTP
// Range requests (`rustic-media://` on desktop, `/api/asset` on the web build),
// so there is no size cap — unlike `read_file_base64`, which is limited to
// 100 MB and round-trips the whole file through IPC as base64.
import { convertFileSrc } from '@tauri-apps/api/core';

const CHUNK = 4 * 1024 * 1024;
const PARALLEL = 4;

/** Streaming URL for `path`; `version` busts caches after the file changes on disk. */
export function fileStreamUrl(path, version) {
  const base = convertFileSrc(path, 'rustic-media');
  if (version === undefined || version === null) return base;
  return `${base}${base.includes('?') ? '&' : '?'}v=${version}`;
}

async function rangeFetch(url, start, end) {
  const res = await fetch(url, { headers: { Range: `bytes=${start}-${end}` } });
  if (!res.ok) throw new Error((await res.text().catch(() => '')) || `HTTP ${res.status}`);
  return res;
}

/** Total file size in bytes via a 1-byte Range probe. */
export async function fileSize(path, version) {
  const url = fileStreamUrl(path, version);
  const res = await rangeFetch(url, 0, 0);
  const total = res.headers.get('content-range')?.split('/')[1];
  await res.arrayBuffer().catch(() => {});
  if (total && total !== '*') return Number(total);
  const len = Number(res.headers.get('content-length'));
  return Number.isFinite(len) ? len : 0;
}

/** Bytes `[start, end)` of `path` (end exclusive), pulled in ≤4 MB Range reads. */
export async function fetchFileRange(path, start, end, { version } = {}) {
  const url = fileStreamUrl(path, version);
  const out = new Uint8Array(Math.max(0, end - start));
  let pos = start;
  while (pos < end) {
    const res = await rangeFetch(url, pos, Math.min(end, pos + CHUNK) - 1);
    const buf = new Uint8Array(await res.arrayBuffer());
    if (buf.length === 0) throw new Error('Unexpected end of file while reading');
    out.set(buf.subarray(0, end - pos), pos - start);
    pos += buf.length;
  }
  return out;
}

/**
 * Reads the whole file into a Uint8Array, chunk by chunk (each response is
 * capped server-side), so files of any size load without hitting IPC limits.
 */
export async function fetchFileBytes(path, { version, onProgress } = {}) {
  const url = fileStreamUrl(path, version);
  const total = await fileSize(path, version);
  const out = new Uint8Array(total);
  if (total === 0) return out;
  const starts = [];
  for (let s = 0; s < total; s += CHUNK) starts.push(s);
  let done = 0;
  let next = 0;
  const worker = async () => {
    while (next < starts.length) {
      const start = starts[next++];
      const end = Math.min(start + CHUNK, total) - 1;
      let pos = start;
      // A server may still return fewer bytes than asked; keep pulling.
      while (pos <= end) {
        const res = await rangeFetch(url, pos, end);
        const buf = new Uint8Array(await res.arrayBuffer());
        if (buf.length === 0) throw new Error('Unexpected end of file while reading');
        out.set(buf, pos);
        pos += buf.length;
        done += buf.length;
        onProgress?.(done, total);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(PARALLEL, starts.length) }, worker));
  return out;
}
