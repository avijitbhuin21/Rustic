// Web shim for `@tauri-apps/plugin-dialog`.
//
// On the desktop these are native OS pickers. In the browser the filesystem the
// user is browsing lives on the SERVER, so `open()` is routed to an in-app
// folder picker (a real file manager backed by the server's read_dir / create /
// delete commands) via the dialog bridge. `save` falls back to a prompt;
// `confirm`/`message`/`ask` map onto the browser dialogs.

import { requestOpenDialog } from './dialog-bridge';

export async function open(options = {}) {
  const selected = await requestOpenDialog(options);
  if (selected == null) return null;
  return options.multiple ? [selected] : selected;
}

export async function save(options = {}) {
  const hint = options.title || 'Enter the absolute save path on the server';
  const value = window.prompt(hint, options.defaultPath || '');
  return value || null;
}

export async function message(msg, _options) {
  window.alert(typeof msg === 'string' ? msg : msg?.message ?? '');
}

export async function confirm(msg, _options) {
  return window.confirm(typeof msg === 'string' ? msg : msg?.message ?? '');
}

export async function ask(msg, _options) {
  return window.confirm(typeof msg === 'string' ? msg : msg?.message ?? '');
}
