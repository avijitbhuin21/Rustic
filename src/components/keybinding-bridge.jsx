// Global keybinding dispatcher. Listens for keydown at the document capture
// phase so user-configured shortcuts fire before sub-components install
// their own handlers (Monaco etc.). When a key matches a bound command we
// run it and stop the event so the underlying widget doesn't also react.
//
// Built-in shortcuts in components that we don't own (Monaco's Ctrl+S,
// useUiZoom's Ctrl+= etc.) still work because most COMMANDS dispatch the
// same synthetic event those handlers already listen for.

import { useEffect } from 'react';
import { useSettings } from '@/state/settings';
import {
  COMMAND_BY_ID,
  buildKeyMap,
  eventToKey,
  isTypingTarget,
} from '@/lib/commands';

// A key is "safe to swallow inside typing targets" only if it uses a modifier
// that isn't shift alone. Pure printable keys / Shift+letter must pass through
// to the input so people can actually type.
function isModifiedShortcut(combo) {
  return /(^|\+)(ctrl|alt|meta)(\+|$)/.test(combo);
}

export function KeybindingBridge() {
  const keybindings = useSettings((s) => s.settings?.keybindings);

  useEffect(() => {
    const map = buildKeyMap(keybindings);
    if (map.size === 0) return;

    const onKey = (e) => {
      // Skip events we synthesized ourselves (see commands.js → dispatchKey).
      // Without this, a command that re-emits its own key produces an
      // infinite loop.
      if (e.__rusticSynthetic) return;
      const key = eventToKey(e);
      if (!key) return;
      const id = map.get(key);
      if (!id) return;
      const cmd = COMMAND_BY_ID[id];
      if (!cmd?.run) return;

      if (isTypingTarget(e.target) && !isModifiedShortcut(key)) return;

      e.preventDefault();
      e.stopPropagation();
      try { cmd.run(); } catch (err) {
        console.error(`[keybinding] command "${id}" failed:`, err);
      }
    };

    document.addEventListener('keydown', onKey, true);
    return () => document.removeEventListener('keydown', onKey, true);
  }, [keybindings]);

  return null;
}
