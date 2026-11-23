// Inline editor chat (Ctrl+I) preferences. The inline chat keeps its own
// model pick, separate from the agent panel's: changing one never changes the
// other. Until a model is picked here it borrows the agent panel's current
// model so the first Ctrl+I works without setup.

import { useMemo } from 'react';
import { create } from 'zustand';
import { useAgent } from '@/state/agent';

const MODEL_PICK_KEY = 'rustic.inlineChat.modelPick';

function loadModelPick() {
  if (typeof window === 'undefined' || !window.localStorage) return null;
  try {
    const parsed = JSON.parse(window.localStorage.getItem(MODEL_PICK_KEY) || 'null');
    if (!parsed || typeof parsed.provider !== 'string' || typeof parsed.modelId !== 'string') return null;
    return { provider: parsed.provider, modelId: parsed.modelId };
  } catch {
    return null;
  }
}

export const useInlineChat = create((set) => ({
  modelPick: loadModelPick(),
  setModelPick: (provider, modelId) => {
    if (!provider || !modelId) return;
    try {
      window.localStorage.setItem(MODEL_PICK_KEY, JSON.stringify({ provider, modelId }));
    } catch {}
    set({ modelPick: { provider, modelId } });
  },
}));

/** The model inline chat runs on: its own remembered pick, else the agent panel's current model. */
export function useInlineChatModel() {
  const pick = useInlineChat((s) => s.modelPick);
  const agentProvider = useAgent((s) => s.selectedProvider);
  const agentModel = useAgent((s) => s.selectedModel);
  return useMemo(() => {
    if (pick) return pick;
    if (agentProvider && agentModel) return { provider: agentProvider, modelId: agentModel };
    return null;
  }, [pick, agentProvider, agentModel]);
}
