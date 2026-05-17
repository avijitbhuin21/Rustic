/**
 * Create a DOM element with attributes and children.
 * el('div', { class: 'foo', onclick: handler }, [child1, child2])
 * el('span', {}, 'text content')
 */
export function el(tag, attrs = {}, children = []) {
  const element = document.createElement(tag);

  for (const [key, value] of Object.entries(attrs)) {
    if (key.startsWith('on') && typeof value === 'function') {
      element.addEventListener(key.slice(2).toLowerCase(), value);
    } else if (key === 'dataset') {
      for (const [dk, dv] of Object.entries(value)) {
        element.dataset[dk] = dv;
      }
    } else if (key === 'style' && typeof value === 'object') {
      Object.assign(element.style, value);
    } else {
      element.setAttribute(key, value);
    }
  }

  if (typeof children === 'string') {
    element.textContent = children;
  } else if (children instanceof Node) {
    element.appendChild(children);
  } else if (Array.isArray(children)) {
    for (const child of children) {
      if (typeof child === 'string') {
        element.appendChild(document.createTextNode(child));
      } else if (child instanceof Node) {
        element.appendChild(child);
      }
    }
  }

  // A11y: if a button/link has a title and no accessible text content, copy
  // the title onto aria-label so screen readers announce something. `title`
  // alone is unreliable on Windows screen readers. We do this in el() so we
  // don't have to touch every icon-button call site.
  if ((tag === 'button' || tag === 'a') && attrs.title && !attrs['aria-label']) {
    const hasTextContent = (element.textContent || '').trim().length > 0;
    if (!hasTextContent) {
      element.setAttribute('aria-label', attrs.title);
    }
  }

  return element;
}

/**
 * Mount an element into a container, replacing existing contents.
 */
export function mount(container, element) {
  container.innerHTML = '';
  container.appendChild(element);
}

/**
 * Create an inline SVG icon from path data.
 */
export function icon(pathData, size = 16) {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('width', size);
  svg.setAttribute('height', size);
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '2');
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');

  const path = document.createElementNS(ns, 'path');
  path.setAttribute('d', pathData);
  svg.appendChild(path);

  return svg;
}

/**
 * Run `callback` once when `element` is removed from the DOM.
 *
 * Previously every caller spun up its own `MutationObserver` on
 * `document.body` with `subtree:true` — that observer wakes on every DOM
 * change anywhere in the app (hundreds per second during streaming chat
 * re-renders), just to check whether one element is still attached.
 *
 * This shared helper uses a single 1s interval to poll `isConnected` for
 * all registered elements. The interval is created lazily on the first
 * registration and torn down when the watch set drains — so when nothing
 * is watching, there is no cost at all.
 *
 * Returns a function that cancels the watch (useful for components that
 * unsubscribe manually before unmount).
 */
const detachWatchers = new Map();
let detachTimer = null;

function tickDetachWatchers() {
  if (detachWatchers.size === 0) {
    if (detachTimer !== null) {
      clearInterval(detachTimer);
      detachTimer = null;
    }
    return;
  }
  for (const [target, cb] of detachWatchers) {
    if (!target.isConnected) {
      detachWatchers.delete(target);
      try { cb(); } catch {}
    }
  }
  if (detachWatchers.size === 0 && detachTimer !== null) {
    clearInterval(detachTimer);
    detachTimer = null;
  }
}

export function onDetached(element, callback) {
  if (!element || typeof callback !== 'function') return () => {};
  detachWatchers.set(element, callback);
  if (detachTimer === null) {
    detachTimer = setInterval(tickDetachWatchers, 1000);
  }
  return () => { detachWatchers.delete(element); };
}

/**
 * Create SVG icon with multiple paths.
 */
export function iconMulti(paths, size = 16) {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('width', size);
  svg.setAttribute('height', size);
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '2');
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');

  for (const d of paths) {
    const path = document.createElementNS(ns, 'path');
    path.setAttribute('d', d);
    svg.appendChild(path);
  }

  return svg;
}
