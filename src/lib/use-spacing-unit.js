import { useSyncExternalStore } from 'react';

const DEFAULT_UNIT = 4;

function read() {
  const cs = getComputedStyle(document.documentElement);
  const raw = cs.getPropertyValue('--spacing').trim();
  const n = parseFloat(raw);
  if (!Number.isFinite(n) || n <= 0) return DEFAULT_UNIT;
  return raw.endsWith('rem') ? n * (parseFloat(cs.fontSize) || 16) : n;
}

function subscribe(onChange) {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-rt-density'] });
  return () => observer.disconnect();
}

/**
 * Px size of one Tailwind spacing unit (`h-6` is six of them). A theme's
 * density changes it, so anything that needs a row height in px — virtual
 * lists — must derive it from this instead of assuming 4.
 */
export function useSpacingUnit() {
  return useSyncExternalStore(subscribe, read, () => DEFAULT_UNIT);
}
