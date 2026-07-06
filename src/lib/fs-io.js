// Scoped filesystem helpers. Frontend writes go through the Rust
// `write_file_base64` command (path-scope validated) instead of the Tauri fs
// plugin, so the broad `fs:**` capability is no longer needed (SEC-01).

import { invoke } from '@tauri-apps/api/core';

function bytesToBase64(bytes) {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export async function writeTextFileScoped(path, contents) {
  /** Write UTF-8 text to disk via the path-scope-validated backend command. */
  const data = bytesToBase64(new TextEncoder().encode(contents));
  await invoke('write_file_base64', { path, data });
}
