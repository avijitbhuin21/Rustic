/**
 * Shared flag that file-node.jsx sets when a context menu action fires,
 * so file-tree.jsx's handleActivate can ignore the tree-library-driven
 * activation that comes in on the same tick (Radix closes the menu and
 * refocuses the trigger row, which react-arborist misreads as a click →
 * activate → open file).
 *
 * The flag MUST be transient. Earlier code set `active = true` directly in
 * every handler but only one of them ever reset it, so the first time a user
 * ran New File / Rename / Delete / Copy / Cut / Paste / Reveal / Open Terminal
 * the flag latched to `true` permanently and `handleActivate` early-returned
 * on every subsequent file click — files silently stopped opening until a full
 * reload reinitialised this module. Always go through `suppressActivate()` so
 * the flag auto-clears and can never get stuck.
 */
let _resetTimer = null;

export const contextMenuState = {
  active: false,

  // Suppress the single spurious activation that fires on the same tick a
  // context-menu item is selected, then auto-clear. The 150ms window covers
  // Radix's deferred focus-restore (some menu items also defer their own work
  // to a setTimeout(0)) while being short enough that it never interferes with
  // a real user click.
  suppressActivate() {
    this.active = true;
    if (_resetTimer) clearTimeout(_resetTimer);
    _resetTimer = setTimeout(() => {
      this.active = false;
      _resetTimer = null;
    }, 150);
  },
};
