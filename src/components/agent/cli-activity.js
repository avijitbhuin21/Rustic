import { useEffect, useState } from 'react';
import { terminalLastOutputAt } from '@/state/terminal';

// A TUI agent repaints continuously while it works and goes quiet at its
// prompt, so "output within the last beat" is a good proxy for "still working".
// The window is generous enough to bridge the gap between two spinner frames.
const BUSY_WINDOW_MS = 1200;
const POLL_MS = 400;

/**
 * True while a CLI agent's PTY is still producing output, i.e. the tool is
 * working rather than waiting for input.
 */
export function useCliBusy(ptySessionId) {
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (ptySessionId == null) {
      setBusy(false);
      return;
    }
    const tick = () => setBusy(Date.now() - terminalLastOutputAt(ptySessionId) < BUSY_WINDOW_MS);
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => clearInterval(id);
  }, [ptySessionId]);

  return busy;
}
