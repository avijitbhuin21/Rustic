// Code-block copy buttons used in chat assistant messages.
//
// `attachCodeCopyButtons(container)` scans for <pre> blocks inside `container`
// and injects a small copy icon. Idempotent — already-decorated blocks are
// skipped, so it's safe to call after partial DOM updates during streaming.

export function attachCodeCopyButtons(container) {
  container.querySelectorAll('pre').forEach((pre) => {
    if (pre.querySelector('.code-copy-btn')) return; // already added

    const code = pre.querySelector('code');
    const textToCopy = (code ?? pre).textContent ?? '';

    const btn = document.createElement('button');
    btn.className = 'code-copy-btn';
    btn.title = 'Copy code';
    btn.setAttribute('aria-label', 'Copy code');

    const copyIcon  = 'M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z';
    const checkIcon = 'M5 13l4 4L19 7';

    function setIcon(path) {
      btn.innerHTML = '';
      const ns = 'http://www.w3.org/2000/svg';
      const svg = document.createElementNS(ns, 'svg');
      svg.setAttribute('width', '13'); svg.setAttribute('height', '13');
      svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('fill', 'none');
      svg.setAttribute('stroke', 'currentColor'); svg.setAttribute('stroke-width', '2');
      svg.setAttribute('stroke-linecap', 'round'); svg.setAttribute('stroke-linejoin', 'round');
      const p = document.createElementNS(ns, 'path');
      p.setAttribute('d', path);
      svg.appendChild(p);
      btn.appendChild(svg);
    }

    setIcon(copyIcon);

    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      navigator.clipboard.writeText(textToCopy).catch(() => {});
      setIcon(checkIcon);
      btn.classList.add('code-copy-btn--copied');
      setTimeout(() => {
        setIcon(copyIcon);
        btn.classList.remove('code-copy-btn--copied');
      }, 1500);
    });

    pre.appendChild(btn);
  });
}
