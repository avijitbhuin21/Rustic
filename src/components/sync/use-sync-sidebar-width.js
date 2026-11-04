// Cloud & Sync takes the whole work area (editor + chat pushed to zero width,
// still mounted) while it's open, and puts the sidebar back to the user's own
// width afterwards.
import { useEffect, useRef } from 'react';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';

const WIDE = 100;

/** `{ panelRef, wide }` for the sidebar `ResizablePanel`. */
export function useSyncSidebarWidth() {
  const panelRef = useRef(null);
  const savedPct = useRef(null);
  const wide = useLayout((s) => s.sidebarVisible && s.activeSidebarPanel === SIDEBAR_PANELS.SYNC);

  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    // Let the new maxSize apply before resizing past the old cap.
    const id = requestAnimationFrame(() => {
      try {
        if (wide) {
          const cur = panel.getSize()?.asPercentage;
          if (savedPct.current == null && cur != null && cur < WIDE) savedPct.current = cur;
          panel.resize(`${WIDE}%`);
        } else if (savedPct.current != null) {
          panel.resize(`${savedPct.current}%`);
          savedPct.current = null;
        }
      } catch { /* panel unmounted mid-frame */ }
    });
    return () => cancelAnimationFrame(id);
  }, [wide]);

  return { panelRef, wide };
}
