import { useSyncExternalStore } from 'react';

const PHONE_MAX = 639;
const TABLET_MAX = 1023;

function query() {
  if (typeof window === 'undefined') return TABLET_MAX + 1;
  return window.innerWidth;
}

function subscribe(callback) {
  if (typeof window === 'undefined') return () => {};
  window.addEventListener('resize', callback);
  window.addEventListener('orientationchange', callback);
  return () => {
    window.removeEventListener('resize', callback);
    window.removeEventListener('orientationchange', callback);
  };
}

/** Returns the live viewport width, re-rendering on resize/orientation change. */
export function useViewportWidth() {
  return useSyncExternalStore(subscribe, query, () => TABLET_MAX + 1);
}

/** Classifies the current viewport into phone / tablet / desktop buckets. */
export function useBreakpoint() {
  const width = useViewportWidth();
  const isPhone = width <= PHONE_MAX;
  const isTablet = width > PHONE_MAX && width <= TABLET_MAX;
  return {
    width,
    isPhone,
    isTablet,
    isDesktop: width > TABLET_MAX,
    isMobile: isPhone || isTablet,
  };
}
