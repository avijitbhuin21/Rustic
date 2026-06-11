// Desktop-only: stream externally-dropped `File`s to disk through Tauri IPC.
//
// Why this exists: `dragDropEnabled` is false in tauri.conf.json (Tauri's
// native drag-drop hook breaks ALL HTML5 drag-and-drop on WebView2, which the
// editor tab DnD depends on), so an OS file dragged into the window arrives as
// a standard HTML5 drop — `File` objects with bytes but no filesystem path.
// We read each file in slices and append the raw bytes via `write_upload_chunk`
// (raw IPC body — no JSON or base64 inflation), so a multi-GB video never has
// to fit in memory.
//
// Call sites gate on `!IS_WEB`; the web build uses lib/file-transfer.js (HTTP
// upload to rustic-server) instead.
import { invoke } from '@tauri-apps/api/core';

const CHUNK_BYTES = 8 * 1024 * 1024;

/// Upload one dropped `File` into `dstDir`. Returns the final absolute path
/// (auto-renamed `foo (1).mp4`-style when the name already exists).
export async function uploadDroppedFile(dstDir, file) {
  const dest = await invoke('begin_drop_upload', {
    dstDir,
    fileName: file.name,
  });
  // Path rides in a header (raw-body invokes carry no JSON args); URI-encode
  // so non-ASCII path characters survive the ASCII-only header transport.
  const headers = { 'x-file-path': encodeURIComponent(dest) };
  for (let offset = 0; offset < file.size; offset += CHUNK_BYTES) {
    const slice = file.slice(offset, Math.min(offset + CHUNK_BYTES, file.size));
    const buf = new Uint8Array(await slice.arrayBuffer());
    await invoke('write_upload_chunk', buf, { headers });
  }
  return dest;
}

/// Upload a list of dropped `File`s into `dstDir`. Returns the count uploaded.
/// Sequential on purpose — parallel multi-GB writes would just thrash the disk.
export async function uploadDroppedFiles(dstDir, files) {
  let count = 0;
  for (const file of files) {
    await uploadDroppedFile(dstDir, file);
    count += 1;
  }
  return count;
}
