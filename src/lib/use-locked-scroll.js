import { useEffect } from 'react';

const LINE_PX = 16;

/**
 * Scrolls `selector` inside `ref` by hand for wheel / touch input. A popover
 * portalled out of a Radix Dialog sits outside the dialog's scroll lock shard,
 * so the lock can swallow the first wheel/touch gestures until the list is
 * focused. Driving scrollTop directly (non-passive, preventDefault) makes the
 * list scroll immediately regardless of which lock is active.
 */
export function useLockedScroll(ref, selector, enabled = true) {
  useEffect(() => {
    const root = ref.current;
    if (!root || !enabled) return undefined;
    const target = () => root.querySelector(selector);

    const onWheel = (e) => {
      const el = target();
      if (!el || el.scrollHeight <= el.clientHeight) return;
      const dy = e.deltaMode === 1 ? e.deltaY * LINE_PX : e.deltaMode === 2 ? e.deltaY * el.clientHeight : e.deltaY;
      el.scrollTop += dy;
      e.preventDefault();
      e.stopPropagation();
    };

    let lastY = null;
    const onTouchStart = (e) => { lastY = e.touches[0]?.clientY ?? null; };
    const onTouchMove = (e) => {
      const el = target();
      const y = e.touches[0]?.clientY;
      if (!el || lastY == null || y == null || el.scrollHeight <= el.clientHeight) return;
      el.scrollTop += lastY - y;
      lastY = y;
      e.preventDefault();
      e.stopPropagation();
    };
    const onTouchEnd = () => { lastY = null; };

    root.addEventListener('wheel', onWheel, { passive: false });
    root.addEventListener('touchstart', onTouchStart, { passive: true });
    root.addEventListener('touchmove', onTouchMove, { passive: false });
    root.addEventListener('touchend', onTouchEnd, { passive: true });
    return () => {
      root.removeEventListener('wheel', onWheel);
      root.removeEventListener('touchstart', onTouchStart);
      root.removeEventListener('touchmove', onTouchMove);
      root.removeEventListener('touchend', onTouchEnd);
    };
  }, [ref, selector, enabled]);
}
