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
