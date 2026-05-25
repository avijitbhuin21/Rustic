import React, { useEffect } from 'react';
import { motion } from 'framer-motion';
import { useAgent } from '@/state/agent';
import { ChatView, PROMPT_SPRING } from './chat-view';
import { PermissionPrompt } from './permission-prompt';
import { QuestionPrompt } from './question-prompt';

// AgentPanel — the chat dock. The entry animation uses the exact same
// PROMPT_SPRING that ChatView's PromptBox uses for its hero↔docked layoutId
// morph. Sharing the spring means the outer panel and the inner input move
// on identical curves at identical speed, so they read as one motion rather
// than two animations running at different rates.
//
// Caveat: in `tauri dev` (HMR + StrictMode + devtools) every layout-branch
// change in MainArea triggers a 200-450ms remount of this subtree which
// will stall any JS-driven animation. In prod that cost drops to tens of
// ms and the spring plays smoothly. The durable fix if perf ever regresses
// is to lift AgentPanel out of MainArea so it stays mounted across layout
// branches — then we could `layout`-animate the size change itself, which
// would also smooth the "expand to full width" direction (currently that
// snaps because React unmounts the docked panel and mounts a new full-
// width one in a different tree).
export default function AgentPanel() {
  const loadInitial = useAgent((s) => s.loadInitial);
  const bindListeners = useAgent((s) => s.bindListeners);

  useEffect(() => {
    loadInitial();
    let cleanup;
    bindListeners().then((fn) => {
      cleanup = fn;
    });
    return () => {
      if (typeof cleanup === 'function') cleanup();
    };
  }, [loadInitial, bindListeners]);

  return (
    <motion.div
      initial={{ opacity: 0, x: 24 }}
      animate={{ opacity: 1, x: 0 }}
      transition={PROMPT_SPRING}
      style={{ willChange: 'transform, opacity' }}
      className="flex h-full flex-col bg-sidebar"
    >
      <ChatView />
      <PermissionPrompt />
      <QuestionPrompt />
    </motion.div>
  );
}
