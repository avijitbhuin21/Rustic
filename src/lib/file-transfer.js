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
export async function uploadFile(dstDir, file, relativePath = null, onProgress = null) {
  const t = await transport();
  return t.uploadFile(dstDir, file, relativePath, onProgress);
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
/// under `dstDir`. Batches over ~8MB show a live progress toast.
export async function uploadFileList(dstDir, files, { preserveTree = false } = {}) {
  const totalBytes = files.reduce((sum, f) => sum + f.size, 0);
  const showProgress = totalBytes > 8 * 1024 * 1024;
  const toastId = 'rustic-upload-progress';
  const { toast } = showProgress ? await import('sonner') : {};

  let count = 0;
  let doneBytes = 0;
  try {
    for (const file of files) {
      const rel = preserveTree ? file.webkitRelativePath || null : null;
      const onProgress = showProgress
        ? (uploaded, total) => {
            const overall = doneBytes + uploaded;
            const pct = totalBytes > 0 ? Math.round((overall / totalBytes) * 100) : 100;
            toast.loading(
              `Uploading ${file.name} — ${formatBytes(overall)} / ${formatBytes(totalBytes)} (${pct}%)`,
              { id: toastId, duration: Infinity },
            );
          }
        : null;
      if (onProgress) onProgress(0, file.size);
      await uploadFile(dstDir, file, rel, onProgress);
      doneBytes += file.size;
      count += 1;
    }
  } finally {
    if (showProgress) toast.dismiss(toastId);
  }
  return count;
}

/// Render a byte count as a short human-readable size (e.g. "1.4 GB").
function formatBytes(n) {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  if (n >= 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${n} B`;
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
