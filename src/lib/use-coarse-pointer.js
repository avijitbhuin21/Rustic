import { useSyncExternalStore } from 'react';

const QUERY = '(pointer: coarse)';

function subscribe(callback) {
  if (typeof window === 'undefined' || !window.matchMedia) return () => {};
  const mql = window.matchMedia(QUERY);
  mql.addEventListener('change', callback);
  return () => mql.removeEventListener('change', callback);
}

function snapshot() {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia(QUERY).matches;
}

/** True when the primary pointer is coarse (touch/pen) — includes touchscreen desktops. */
export function useCoarsePointer() {
  return useSyncExternalStore(subscribe, snapshot, () => false);
}

/** Non-reactive read of the coarse-pointer state, for event handlers and imperative code. */
export function isCoarsePointer() {
  return snapshot();
}
