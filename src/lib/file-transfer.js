// Browser-only file upload / download helpers for the explorer.
//
// These are gated behind `IS_WEB` at the call sites (the desktop app reaches
// the host filesystem directly), so this module only does real work in the web
// build. It lazily imports the web transport so the desktop bundle never pulls
// the fetch/FileReader code path.
import { IS_WEB } from './platform.js';

async function transport() {
  if (!IS_WEB) {
    throw new Error('file-transfer is only available in the web build');
  }
  return import('./web/transport-core.js');
}

/// Download a server-side file (raw) or folder (as a zip) to the browser.
export async function downloadPath(path) {
  const t = await transport();
  return t.downloadPath(path);
}

/// Upload a single browser `File` into `dstDir`, optionally preserving a
/// `relativePath` subtree (folder uploads).
export async function uploadFile(dstDir, file, relativePath = null) {
  const t = await transport();
  return t.uploadFile(dstDir, file, relativePath);
}

/// Open the OS file picker and upload every chosen file into `dstDir`.
/// Returns the number of files uploaded.
export async function pickAndUploadFiles(dstDir) {
  const files = await pickFiles({ directory: false });
  return uploadFileList(dstDir, files);
}

/// Open the OS folder picker and upload the whole tree into `dstDir`,
/// preserving each file's relative path. Returns the number of files uploaded.
export async function pickAndUploadFolder(dstDir) {
  const files = await pickFiles({ directory: true });
  return uploadFileList(dstDir, files, { preserveTree: true });
}

/// Upload an already-collected list of `File`s (e.g. from a drag-drop event).
/// When `preserveTree` is set, each file's `webkitRelativePath` is recreated
/// under `dstDir`.
export async function uploadFileList(dstDir, files, { preserveTree = false } = {}) {
  let count = 0;
  for (const file of files) {
    const rel = preserveTree ? file.webkitRelativePath || null : null;
    await uploadFile(dstDir, file, rel);
    count += 1;
  }
  return count;
}

function pickFiles({ directory }) {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = true;
    if (directory) {
      input.webkitdirectory = true;
      input.directory = true;
    }
    input.style.display = 'none';
    document.body.appendChild(input);
    input.addEventListener('change', () => {
      const files = Array.from(input.files || []);
      input.remove();
      resolve(files);
    });
    // If the dialog is cancelled there's no 'change' event; clean up on focus
    // return so we don't leak the input element.
    const onFocus = () => {
      setTimeout(() => {
        if (document.body.contains(input) && (!input.files || input.files.length === 0)) {
          input.remove();
          resolve([]);
        }
        window.removeEventListener('focus', onFocus);
      }, 300);
    };
    window.addEventListener('focus', onFocus);
    input.click();
  });
}
