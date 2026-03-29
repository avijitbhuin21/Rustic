import { el } from '../utils/dom.js';

/**
 * Creates a dropdown menu that attaches to a trigger element.
 * @param {Array<{label, shortcut?, action?, separator?, disabled?}>} items
 * @returns {{ element, show(x, y), hide() }}
 */
export function createDropdownMenu(items) {
  const menu = el('div', { class: 'dropdown-menu' });
  menu.style.display = 'none';

  for (const item of items) {
    if (item.separator) {
      menu.appendChild(el('div', { class: 'dropdown-menu__separator' }));
      continue;
    }

    const row = el('div', {
      class: `dropdown-menu__item ${item.disabled ? 'dropdown-menu__item--disabled' : ''}`,
    });
    row.appendChild(el('span', { class: 'dropdown-menu__label' }, item.label));
    if (item.shortcut) {
      row.appendChild(el('span', { class: 'dropdown-menu__shortcut' }, item.shortcut));
    }
    if (!item.disabled && item.action) {
      row.addEventListener('click', (e) => {
        e.stopPropagation();
        hide();
        item.action();
      });
    }
    menu.appendChild(row);
  }

  function show(x, y) {
    // Place off-screen first to measure dimensions
    menu.style.left = '0px';
    menu.style.top = '-9999px';
    menu.style.display = 'block';

    const menuW = menu.offsetWidth;
    const menuH = menu.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    // Clamp so menu never goes outside the viewport
    const clampedX = Math.max(0, Math.min(x, vw - menuW - 4));
    const clampedY = Math.max(0, Math.min(y, vh - menuH - 4));

    menu.style.left = `${clampedX}px`;
    menu.style.top = `${clampedY}px`;

    // Close on outside click
    const onOutsideClick = (e) => {
      if (!menu.contains(e.target)) {
        hide();
        document.removeEventListener('click', onOutsideClick, true);
      }
    };
    setTimeout(() => document.addEventListener('click', onOutsideClick, true), 0);
  }

  function hide() {
    menu.style.display = 'none';
  }

  return { element: menu, show, hide };
}

/**
 * Show a context menu at a position.
 */
export function showContextMenu(items, x, y) {
  // Remove any existing context menu
  document.querySelectorAll('.dropdown-menu--context').forEach((el) => el.remove());

  const menu = createDropdownMenu(items);
  menu.element.classList.add('dropdown-menu--context');
  document.body.appendChild(menu.element);
  menu.show(x, y);

  return menu;
}
