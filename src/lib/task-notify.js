import { toast } from 'sonner';
import { invoke } from '@tauri-apps/api/core';
import { IS_WEB, isTauriAvailable } from '@/lib/platform';
import { useSettings } from '@/state/settings';

const DEDUPE_MS = 3000;
const recent = new Map();
let permission = null;

const COPY = {
  completed: { verb: 'finished', toast: 'success' },
  failed: { verb: 'failed', toast: 'error' },
  input: { verb: 'needs your input', toast: 'info' },
  goal: { verb: 'met its goal', toast: 'success' },
};

/** True when the setting is on (missing = on, so old settings files notify). */
function enabled() {
  return useSettings.getState().settings?.general?.notify_task_events !== false;
}

/** Ask for OS notification permission once; resolves to whether we may notify. */
async function ensurePermission() {
  if (permission != null) return permission;
  try {
    if (IS_WEB) {
      if (typeof Notification === 'undefined') return (permission = false);
      if (Notification.permission === 'default') await Notification.requestPermission();
      permission = Notification.permission === 'granted';
    } else if (isTauriAvailable()) {
      let granted = await invoke('plugin:notification|is_permission_granted');
      if (granted == null) granted = (await invoke('plugin:notification|request_permission')) === 'granted';
      permission = !!granted;
    } else {
      permission = false;
    }
  } catch {
    permission = false;
  }
  return permission;
}

/** Show a native OS notification; clicking it (web only) focuses the window and runs `onOpen`. */
async function systemNotify(title, body, onOpen) {
  if (!(await ensurePermission())) return;
  try {
    if (IS_WEB) {
      const n = new Notification(title, { body, tag: title });
      n.onclick = () => { window.focus(); onOpen?.(); n.close(); };
    } else {
      await invoke('plugin:notification|notify', { options: { title, body } });
    }
  } catch { /* notifications are best-effort */ }
}

/** Notify about a chat event unless the user is already looking at that chat. */
export function notifyTaskEvent({ taskId, kind, title, detail, isViewing, onOpen, inApp = true }) {
  if (!taskId || !COPY[kind] || !enabled()) return;
  const key = `${taskId}:${kind}`;
  const now = Date.now();
  if (now - (recent.get(key) || 0) < DEDUPE_MS) return;
  recent.set(key, now);

  const focused = typeof document !== 'undefined' && document.hasFocus() && document.visibilityState === 'visible';
  if (focused && isViewing) return;

  const name = (title || '').trim() || 'Chat';
  const { verb, toast: variant } = COPY[kind];
  const heading = `${name} ${verb}`;
  if (inApp) {
    toast[variant](heading, {
      description: detail || undefined,
      duration: 8000,
      action: onOpen ? { label: 'Open', onClick: onOpen } : undefined,
    });
  }
  if (!focused) systemNotify(heading, detail || 'Rustic', onOpen);
}

/** Request OS notification permission early (call from a user gesture on web). */
export function primeNotificationPermission() {
  if (enabled()) ensurePermission();
}
