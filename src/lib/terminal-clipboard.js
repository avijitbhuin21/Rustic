import { toast } from 'sonner';
import {
  extractImagesFromClipboard,
  readFileAsBase64,
  saveImageToUploads,
  uploadsAbsoluteDir,
  pasteOsClipboardImageInto,
} from '@/lib/clipboard-image';

// xterm.js forwards every keystroke straight to the PTY, so Ctrl+V becomes
// the raw ^V (0x16) control byte instead of a clipboard paste. TUI apps like
// `claude` read stdin raw and don't interpret ^V as anything useful.
//
// We intercept Ctrl+V two ways:
//   1. attachCustomKeyEventHandler — suppresses xterm sending ^V to the PTY.
//   2. The browser's `paste` event on the helper textarea — this is where
//      we actually read clipboardData, because it gives us synchronous access
//      to ALL MIME formats (image and text) in one shot. navigator.clipboard
//      can't do that — readText() biases towards text even when an image is
//      present, which is why my first attempt didn't handle screenshots.
//
// Image-first detection: if the clipboard has an image, the user pasted an
// image — full stop. We save it under <cwd>/.rustic/uploaded/<date>/ and
// paste the absolute path so the TUI agent (claude, etc) can `Read` it.

// Run when the browser fires a `paste` event on xterm's helper textarea.
// Returns true once the paste is handled so the caller can preventDefault
// and stopImmediatePropagation against xterm's own paste listener.
export async function handleTerminalPaste(term, sessionCwd, clipboardData) {
  // Image branch first — a screenshot may also carry an empty-ish text/plain
  // value that we'd otherwise paste instead.
  const images = extractImagesFromClipboard(clipboardData);
  if (images.length > 0) {
    if (!sessionCwd) {
      toast.error('Open a project before pasting an image into the terminal.');
      return;
    }
    try {
      const { file, mediaType } = images[0];
      const { base64 } = await readFileAsBase64(file);
      const { absolutePath } = await saveImageToUploads({
        projectRoot: sessionCwd,
        base64,
        dstDir: uploadsAbsoluteDir(sessionCwd),
      });
      term.paste(absolutePath);
      toast.success(`Saved ${mediaType.replace('image/', '')} — pasted path`);
    } catch (err) {
      const msg = typeof err === 'string' ? err : err?.message || String(err);
      toast.error(`Image paste failed: ${msg}`);
    }
    return;
  }

  // Text branch — straightforward bracketed paste via xterm.
  const text = clipboardData?.getData('text/plain') || '';
  if (text.length > 0) {
    term.paste(text);
  }
}

// Fallback path when the user hits Ctrl+V but no `paste` event fires (focus
// gymnastics in WebView2, or a future right-click-paste menu). Reads the OS
// clipboard via Tauri so we still catch screenshots in that case.
export async function pasteViaClipboardApi(term, sessionCwd) {
  // Image first, via the OS-side reader that we already use for the file
  // explorer. It returns null when there's no image, so we can fall through
  // to text without an extra round-trip on the failure case.
  if (sessionCwd) {
    try {
      const dstDir = uploadsAbsoluteDir(sessionCwd);
      const saved = await pasteOsClipboardImageInto(dstDir);
      if (saved) {
        term.paste(saved);
        toast.success('Pasted image path');
        return;
      }
    } catch (err) {
      const msg = typeof err === 'string' ? err : err?.message || String(err);
      console.warn('[terminal] OS image paste failed:', msg);
    }
  }

  try {
    const text = await navigator.clipboard.readText();
    if (text && text.length > 0) term.paste(text);
  } catch (err) {
    console.warn('[terminal] navigator.clipboard.readText failed:', err);
  }
}

// Copy current selection to the OS clipboard. Returns true when something
// was actually copied.
export function copyTerminalSelection(term) {
  const sel = term.getSelection();
  if (!sel) return false;
  try {
    navigator.clipboard.writeText(sel);
    return true;
  } catch {
    return false;
  }
}
