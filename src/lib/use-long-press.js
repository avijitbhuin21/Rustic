import { useCallback, useEffect, useRef } from 'react';

const HOLD_MS = 450;
const MOVE_TOLERANCE_PX = 12;

/**
 * Touch long-press that opens the element's own context menu.
 *
 * Radix's ContextMenuTrigger already long-presses, but it cancels on ANY
 * pointermove — a finger always jitters a pixel or two, so in practice the menu
 * never opens on a real touchscreen. This re-implements the gesture with a
 * movement tolerance and dispatches a genuine `contextmenu` event at the touch
 * point, which the existing trigger handles: same menu, same items, no fork.
 *
 * Returns props to spread on the SAME element that carries the trigger.
 */
export function useLongPress({ enabled = true, onLongPress } = {}) {
  const timerRef = useRef(0);
  const originRef = useRef({ x: 0, y: 0 });
  const firedRef = useRef(false);

  const clear = useCallback(() => {
    if (timerRef.current) {
      window.clearTimeout(timerRef.current);
      timerRef.current = 0;
    }
  }, []);

  useEffect(() => clear, [clear]);

  const onPointerDown = useCallback(
    (event) => {
      if (!enabled || event.pointerType === 'mouse' || event.isPrimary === false) return;
      const target = event.currentTarget;
      const { clientX: x, clientY: y } = event;
      originRef.current = { x, y };
      firedRef.current = false;
      clear();
      timerRef.current = window.setTimeout(() => {
        timerRef.current = 0;
        firedRef.current = true;
        // Haptic confirmation where supported; harmless no-op elsewhere.
        try { navigator.vibrate?.(10); } catch {}
        if (onLongPress) {
          onLongPress({ x, y });
          return;
        }
        target?.dispatchEvent(
          new MouseEvent('contextmenu', {
            bubbles: true,
            cancelable: true,
            clientX: x,
            clientY: y,
            button: 2,
          }),
        );
      }, HOLD_MS);
    },
    [enabled, onLongPress, clear],
  );

  const onPointerMove = useCallback(
    (event) => {
      if (!timerRef.current || event.pointerType === 'mouse') return;
      const dx = Math.abs(event.clientX - originRef.current.x);
      const dy = Math.abs(event.clientY - originRef.current.y);
      if (dx > MOVE_TOLERANCE_PX || dy > MOVE_TOLERANCE_PX) clear();
    },
    [clear],
  );

  const onPointerUp = useCallback(() => clear(), [clear]);

  // A long-press must not also count as a tap (open the file, switch the tab…).
  const onClickCapture = useCallback((event) => {
    if (!firedRef.current) return;
    firedRef.current = false;
    event.preventDefault();
    event.stopPropagation();
  }, []);

  return {
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel: onPointerUp,
    onClickCapture,
  };
}
