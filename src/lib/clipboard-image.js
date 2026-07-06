import { invoke } from '@tauri-apps/api/core';

// Helpers for handling pasted/screenshot images in both the agent prompt box
// and the file explorer. Two entry points are used today:
//
//   - extractImagesFromClipboard(e.clipboardData) returns the image File
//     objects out of a browser `paste` event (Ctrl+V inside a focused textarea
//     or contenteditable). Used by the prompt box.
//   - For places where the focus model can't intercept a paste event (e.g.
//     the file tree, where the user just clicks the pane and hits Ctrl+V on
//     a non-editable element), call the OS-level `paste_clipboard_image_into`
//     Tauri command instead — it pulls the bitmap from the OS clipboard via
//     PowerShell/AppleScript/xclip and writes it directly.
//
// Both paths converge on `<project>/.rustic/uploaded/<YYYY-MM-DD>/<file>`
// so the agent can `Read` them by path later. saveImageToUploads builds the
// path and uses `write_file_base64` (auto-creates parent dirs) to persist.

const MEDIA_TYPE_EXT = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/jpg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/bmp': 'bmp',
  'image/svg+xml': 'svg',
};

export function extractImagesFromClipboard(clipboardData) {
  const out = [];
  const items = clipboardData?.items;
  if (!items) return out;
  for (const it of items) {
    if (it.kind !== 'file') continue;
    const mt = it.type || '';
    if (!mt.startsWith('image/')) continue;
    const file = it.getAsFile();
    if (!file) continue;
    out.push({ file, mediaType: mt, ext: MEDIA_TYPE_EXT[mt] || 'png' });
  }
  return out;
}

// Safari (especially on iPad) often exposes a copied image ONLY through the
// async clipboard API — the synchronous `paste` event's clipboardData comes
// back empty for images copied from Photos/Files/other apps. Reading here may
// show Safari's one-tap "Paste" permission bubble, which is expected. Returns
// the same `{ file, mediaType, ext }` shape as extractImagesFromClipboard.
export async function readImagesFromAsyncClipboard() {
  const out = [];
  if (!navigator.clipboard?.read) return out;
  let items;
  try {
    items = await navigator.clipboard.read();
  } catch {
    return out; // permission denied or empty clipboard
  }
  for (const item of items) {
    const mt = (item.types || []).find((t) => t.startsWith('image/'));
    if (!mt) continue;
    try {
      const blob = await item.getType(mt);
      const ext = MEDIA_TYPE_EXT[mt] || 'png';
      const file = new File([blob], `pasted-image.${ext}`, { type: mt });
      out.push({ file, mediaType: mt, ext });
    } catch {
      /* skip unreadable clipboard item */
    }
  }
  return out;
}

// Reverse of MEDIA_TYPE_EXT — infer a mime type from a file path's extension.
// Returns null for non-image extensions.
export function imageMimeFromPath(path) {
  const ext = (path.split('.').pop() || '').toLowerCase();
  for (const [mime, e] of Object.entries(MEDIA_TYPE_EXT)) {
    if (e === ext || (ext === 'jpeg' && e === 'jpg')) return mime;
  }
  return null;
}

export function readFileAsBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = reader.result;
      if (typeof dataUrl !== 'string') {
        reject(new Error('FileReader returned non-string'));
        return;
      }
      const comma = dataUrl.indexOf(',');
      const base64 = comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
      resolve({ base64, dataUrl });
    };
    reader.onerror = () => reject(reader.error || new Error('read failed'));
    reader.readAsDataURL(file);
  });
}

function joinPath(root, ...parts) {
  // Pick whichever separator the project root already uses so the result
  // looks native — agents and the file watcher both prefer matching style.
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  const trimmedRoot = root.replace(/[\\/]+$/, '');
  const tail = parts.map((p) => p.replace(/^[\\/]+|[\\/]+$/g, '')).filter(Boolean).join(sep);
  return `${trimmedRoot}${sep}${tail}`;
}

export function uploadsAbsoluteDir(projectRoot) {
  return joinPath(projectRoot, '.rustic', 'uploaded');
}

// Write a pasted image into `<dst_dir>/pasted-image.png` (or `pasted-image-N.png`
// on collision) and return the absolute + project-relative paths. Used by the
// chat composer to drop pasted screenshots under `<project>/.rustic/uploaded/`,
// but `dst_dir` is generic so callers can target any folder.
export async function saveImageToUploads({ projectRoot, base64, dstDir }) {
  const targetDir = dstDir || (projectRoot && uploadsAbsoluteDir(projectRoot));
  if (!targetDir) {
    throw new Error('No destination — open a project before pasting an image.');
  }
  const absolutePath = await invoke('save_pasted_image_base64', {
    dstDir: targetDir,
    data: base64,
  });
  const filename = absolutePath.split(/[\\/]/).pop() || 'pasted-image.png';
  // Compute a project-relative path for display + agent-side reference. If
  // the caller didn't supply a project root (e.g. pasting into an arbitrary
  // explorer folder outside a project), fall back to the absolute path.
  let relativePath = absolutePath;
  if (projectRoot) {
    const normRoot = projectRoot.replace(/[\\/]+$/, '');
    if (absolutePath.startsWith(normRoot)) {
      relativePath = absolutePath
        .slice(normRoot.length)
        .replace(/^[\\/]+/, '')
        .replace(/\\/g, '/');
    }
  }
  return { absolutePath, relativePath, filename };
}

// OS-level path: pull a screenshot/snip directly off the platform clipboard
// and drop it into the given folder. Used by the file explorer where there's
// no focused editable to intercept a browser paste event. The backend
// auto-creates the destination directory so callers don't have to pre-create
// it. Returns the saved absolute path, or null if the clipboard had no image.
export async function pasteOsClipboardImageInto(dstDir) {
  if (!dstDir) {
    throw new Error('No destination directory provided.');
  }
  const saved = await invoke('paste_clipboard_image_into', { dstDir });
  return saved || null;
}
