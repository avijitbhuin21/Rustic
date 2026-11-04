import { useEffect, useRef } from 'react';

/**
 * Run `fn` immediately and then every `ms` milliseconds, skipping ticks while
 * the window is hidden and running once as soon as it becomes visible again.
 * Pass `ms <= 0` (or a falsy value) to disable polling. `deps` restart the
 * interval; `fn` itself can change freely between renders.
 */
export function useVisiblePoll(fn, ms, deps = []) {
  const fnRef = useRef(fn);
  fnRef.current = fn;

  useEffect(() => {
    if (!ms || ms <= 0) return undefined;
    const tick = () => {
      if (typeof document !== 'undefined' && document.hidden) return;
      fnRef.current();
    };
    tick();
    const id = setInterval(tick, ms);
    document.addEventListener('visibilitychange', tick);
    return () => {
      clearInterval(id);
      document.removeEventListener('visibilitychange', tick);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ms, ...deps]);
}

export default useVisiblePoll;
