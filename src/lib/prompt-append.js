import { useAgent } from '@/state/agent';
import { useLayout } from '@/state/layout';

/** Put `text` in the active chat's box without sending it, and open the chat if it is closed. */
export function appendToPrompt(text) {
  if (typeof text !== 'string' || !text) return;
  // A mounted prompt box claims the event; otherwise the text goes into the stored
  // draft, which the box loads when it mounts.
  const detail = { text, claimed: false };
  window.dispatchEvent(new CustomEvent('prompt-append', { detail }));
  if (!detail.claimed) {
    const s = useAgent.getState();
    const key = s.activeTaskId ? `task:${s.activeTaskId}` : `new:${s.activeProject?.id ?? 'none'}`;
    const prev = s.promptDrafts[key] || {};
    s.setPromptDraft(key, { ...prev, value: prev.value ? `${prev.value}\n\n${text}` : text });
  }
  useLayout.getState().openChatDock();
}
