// Module-level ref to the currently focused Monaco editor + a shared
// "format the open document" routine that the Shortcuts system can call from
// outside React.
//
// monaco-editor.jsx registers/unregisters itself here as editors mount,
// focus, and unmount. The shortcuts bridge (and any other consumer) reads
// `getActiveEditor()` when it needs to act on whatever the user is looking at.

import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { formatWithPrettier, isPrettierLanguage } from '@/lib/prettier-client';

let activeEditor = null;
let activeTab = null;
let activeSaver = null;

export function setActiveEditor(editor, tab) {
  activeEditor = editor;
  activeTab = tab;
}

// monaco-editor.jsx registers its `save()` closure here so file.save (and
// any other consumer) can save without going through Monaco's internal
// keybinding service.
export function setActiveSaver(fn) { activeSaver = fn; }
export function clearActiveSaver(fn) { if (activeSaver === fn) activeSaver = null; }
export async function saveActiveEditor() {
  if (activeSaver) return activeSaver();
}

export function clearActiveEditor(editor) {
  // Only clear if the unmounting editor is still the registered one. Without
  // this guard, a stale unmount cleanup can wipe the entry just as a sibling
  // editor took focus.
  if (activeEditor === editor) {
    activeEditor = null;
    activeTab = null;
  }
}

export function getActiveEditor() {
  return activeEditor;
}

export function getActiveTab() {
  return activeTab;
}

// Apply formatted content via executeEdits so Monaco records the change as an
// undoable operation. Plain editor.setValue() *replaces the model* and wipes
// the undo stack, which means Ctrl+Z after format-on-save can't restore the
// pre-format state — see the bug reported around format-on-save undo.
export function applyFormattedContent(editor, formatted) {
  const model = editor.getModel();
  if (!model) return;
  const pos = editor.getPosition();
  const fullRange = model.getFullModelRange();
  editor.pushUndoStop();
  editor.executeEdits('format', [{
    range: fullRange,
    text: formatted,
    forceMoveMarkers: true,
  }]);
  editor.pushUndoStop();
  if (pos) {
    try { editor.setPosition(pos); } catch {}
  }
}

// Format the currently focused Monaco editor in-place. Mirrors the
// resolution order used by Save: backend formatter → Prettier → Monaco
// built-in. Surfaces toasts on success/failure so the user gets feedback.
export async function formatActiveEditor() {
  const editor = activeEditor;
  const tab = activeTab;
  if (!editor) {
    toast.error('No editor focused — open a file first.');
    return;
  }

  const lang = tab?.language || 'plaintext';
  const source = editor.getValue();
  let formatted = null;

  try {
    const res = await invoke('formatter_format', {
      req: { language: lang, source, file_path: tab?.path ?? null },
    });
    if (res?.formatted !== undefined) formatted = res.formatted;
  } catch (err) {
    const msg = String(err).toLowerCase();
    if (!msg.includes('no formatter configured')) {
      toast.error(`Formatter failed: ${err}`);
    }
  }

  if (formatted === null && isPrettierLanguage(lang)) {
    try {
      formatted = await formatWithPrettier(lang, source, {
        filepath: tab?.path ?? undefined,
      });
    } catch (err) {
      toast.error(`Prettier: ${err.message ?? err}`);
    }
  }

  if (formatted !== null && formatted !== source) {
    applyFormattedContent(editor, formatted);
    toast.success('Formatted');
    return;
  }
  if (formatted === source) {
    toast.message('Already formatted');
    return;
  }

  // No external formatter handled it — fall back to Monaco's built-in.
  try {
    const action = editor.getAction('editor.action.formatDocument');
    if (action) {
      await action.run();
      return;
    }
  } catch (err) {
    toast.error(`Format failed: ${err.message ?? err}`);
    return;
  }
  toast.error(`No formatter available for ${lang}.`);
}

export function toggleCommentActiveEditor() {
  const editor = activeEditor;
  if (!editor) {
    toast.error('No editor focused — open a file first.');
    return;
  }
  const action = editor.getAction('editor.action.commentLine');
  if (action) {
    action.run();
  }
}
