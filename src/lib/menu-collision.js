const EDGE_PADDING = 8;

/**
 * Collision padding for popper menus so they never open flush against a screen
 * edge — or underneath the phone shell's bottom tab bar.
 *
 * Radix's default padding is 0, so a menu opened near the bottom is shifted to
 * end exactly at the layout viewport edge. On a phone that last strip is covered
 * by our own bottom nav (and by the browser's URL / gesture bar), so the final
 * menu items looked cut off and were unreachable. Reserving the nav's measured
 * height also shrinks `--radix-*-content-available-height`, so an over-tall menu
 * scrolls internally instead of running off-screen.
 */
export function menuCollisionPadding() {
  let bottom = EDGE_PADDING;
  if (typeof window !== 'undefined' && typeof document !== 'undefined') {
    const nav = document.querySelector('[data-mobile-nav]');
    if (nav) {
      const rect = nav.getBoundingClientRect();
      // Only a bar actually parked at the bottom edge steals space.
      if (rect.height > 0 && rect.bottom >= window.innerHeight - 2) {
        bottom += rect.height;
      }
    }
  }
  return { top: EDGE_PADDING, right: EDGE_PADDING, bottom, left: EDGE_PADDING };
}
