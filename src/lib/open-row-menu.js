/**
 * Opens a row's existing context menu from an explicit button (the touch "⋯").
 *
 * Radix ContextMenu can only be opened by a `contextmenu` event on its trigger,
 * so rather than duplicating every menu as a DropdownMenu we synthesize that
 * event on the trigger element, anchored at the button. One menu definition
 * serves right-click, long-press and the ⋯ button.
 */
export function openRowMenu(event, triggerSelector) {
  event.preventDefault();
  event.stopPropagation();
  const button = event.currentTarget;
  const trigger = triggerSelector ? button.closest(triggerSelector) : button.parentElement;
  if (!trigger) return;
  const rect = button.getBoundingClientRect();
  trigger.dispatchEvent(
    new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
      clientX: rect.left + rect.width / 2,
      clientY: rect.bottom,
      button: 2,
    }),
  );
}
