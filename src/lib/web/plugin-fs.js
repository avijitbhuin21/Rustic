// Web shim for `@tauri-apps/plugin-fs`.
// Routes filesystem access through the same server command surface the rest of
// the app uses, so reads/writes hit the VM's filesystem (where the projects
// live), not the browser sandbox.

import { invoke } from './transport-core.js';

export async function readTextFile(path, _options) {
  return invoke('read_file_content', { path });
}

export async function readFile(path, _options) {
  const b64 = await invoke('read_file_base64', { path });
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

export async function writeTextFile(path, contents, _options) {
  // `write_file_base64` is the server's binary-write endpoint; encode UTF-8.
  const bytes = new TextEncoder().encode(contents);
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return invoke('write_file_base64', { path, base64: btoa(bin) });
}

export async function exists(path) {
  try {
    const s = await invoke('stat_path', { path });
    return !!(s && s.exists);
  } catch {
    return false;
  }
}

export async function mkdir(_path, _options) {
  throw new Error('fs.mkdir is not wired into the web build yet');
}

export async function remove(path, _options) {
  return invoke('delete_entry', { path });
}
