import { useEffect } from 'react';

let mermaidPromise = null;

/** Lazily loads and initializes the mermaid library once. */
function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then((mod) => {
      const mermaid = mod.default || mod;
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: document.documentElement.classList.contains('dark') ? 'dark' : 'default',
        fontFamily: 'inherit',
      });
      return mermaid;
    });
  }
  return mermaidPromise;
}

/** Parses a CSS color (#rgb, #rrggbb, rgb()) into [r,g,b] or null. */
function parseColor(c) {
  if (!c) return null;
  c = c.trim();
  let m = c.match(/^#([0-9a-f]{3})$/i);
  if (m) {
    return m[1].split('').map((h) => parseInt(h + h, 16));
  }
  m = c.match(/^#([0-9a-f]{6})$/i);
  if (m) {
    return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16));
  }
  m = c.match(/^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/i);
  if (m) return [Number(m[1]), Number(m[2]), Number(m[3])];
  return null;
}

/**
 * Fixes label contrast for nodes the diagram author styled with explicit
 * fills (`style A fill:#e1f5ff`): the dark theme's light label text becomes
 * unreadable on light fills (and vice versa). Walks each node, reads its
 * shape fill, and forces the label to black/white by luminance.
 */
function fixLabelContrast(root) {
  for (const node of root.querySelectorAll('g.node')) {
    const shape = node.querySelector('rect, polygon, circle, ellipse, path');
    if (!shape) continue;
    const fill = shape.style?.fill || shape.getAttribute('fill');
    const rgb = parseColor(fill);
    if (!rgb) continue;
    const luminance = (0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]) / 255;
    const color = luminance > 0.55 ? '#111827' : '#f9fafb';
    for (const t of node.querySelectorAll('text, tspan')) {
      t.setAttribute('fill', color);
      t.style.fill = color;
    }
    for (const s of node.querySelectorAll('foreignObject span, foreignObject div, .label')) {
      s.style.color = color;
    }
  }
}

let renderSeq = 0;

const MAXIMIZE_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/><path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/></svg>';

/**
 * Opens a fullscreen overlay for a rendered mermaid SVG with scroll-wheel
 * zoom (anchored at the cursor), drag-to-pan, and Esc / ✕ / backdrop close.
 */
function openMermaidFullscreen(svgHtml) {
  const overlay = document.createElement('div');
  overlay.className = 'mermaid-fs-overlay';
  const canvas = document.createElement('div');
  canvas.className = 'mermaid-fs-canvas';
  const inner = document.createElement('div');
  inner.className = 'mermaid-fs-inner';
  inner.innerHTML = svgHtml;
  canvas.appendChild(inner);
  const closeBtn = document.createElement('button');
  closeBtn.type = 'button';
  closeBtn.className = 'mermaid-fs-close';
  closeBtn.setAttribute('aria-label', 'Close fullscreen diagram');
  closeBtn.textContent = '✕';
  overlay.appendChild(canvas);
  overlay.appendChild(closeBtn);
  document.body.appendChild(overlay);

  // Give the SVG its natural (viewBox) pixel size so the transform math is
  // stable, then fit-and-center it in the viewport.
  const svg = inner.querySelector('svg');
  let scale = 1;
  let tx = 0;
  let ty = 0;
  const apply = () => {
    inner.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
  };
  if (svg) {
    svg.style.maxWidth = 'none';
    svg.style.maxHeight = 'none';
    const vb = svg.viewBox?.baseVal;
    const w = vb?.width || svg.getBoundingClientRect().width || 800;
    const h = vb?.height || svg.getBoundingClientRect().height || 600;
    svg.style.width = `${w}px`;
    svg.style.height = `${h}px`;
    requestAnimationFrame(() => {
      const cw = canvas.clientWidth;
      const ch = canvas.clientHeight;
      scale = Math.min((cw * 0.92) / w, (ch * 0.92) / h, 2);
      tx = (cw - w * scale) / 2;
      ty = (ch - h * scale) / 2;
      apply();
    });
  }

  canvas.addEventListener(
    'wheel',
    (e) => {
      e.preventDefault();
      e.stopPropagation();
      const factor = Math.exp(-e.deltaY / 400);
      const next = Math.min(12, Math.max(0.1, scale * factor));
      if (next === scale) return;
      const rect = canvas.getBoundingClientRect();
      const cx = e.clientX - rect.left;
      const cy = e.clientY - rect.top;
      // Keep the diagram point under the cursor stationary.
      tx = cx - (cx - tx) * (next / scale);
      ty = cy - (cy - ty) * (next / scale);
      scale = next;
      apply();
    },
    { passive: false },
  );

  let drag = null;
  canvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    drag = { x: e.clientX - tx, y: e.clientY - ty };
    canvas.setPointerCapture?.(e.pointerId);
    canvas.classList.add('dragging');
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!drag) return;
    tx = e.clientX - drag.x;
    ty = e.clientY - drag.y;
    apply();
  });
  const endDrag = () => {
    drag = null;
    canvas.classList.remove('dragging');
  };
  canvas.addEventListener('pointerup', endDrag);
  canvas.addEventListener('pointerleave', endDrag);

  const onKey = (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      close();
    }
  };
  const close = () => {
    document.removeEventListener('keydown', onKey, true);
    overlay.remove();
  };
  document.addEventListener('keydown', onKey, true);
  closeBtn.addEventListener('click', close);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) close();
  });
}

// First-line keywords that identify a mermaid diagram even when the fence
// carried no `mermaid` language tag (models frequently omit it).
const MERMAID_KEYWORDS =
  /^\s*(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram(-v2)?|erDiagram|journey|gantt|pie|mindmap|timeline|gitGraph|quadrantChart|sankey|xychart(-beta)?|block(-beta)?)\b/;

function isMermaidBlock(code) {
  if (code.className?.includes('language-mermaid')) return true;
  // Only content-sniff untagged fences — a tagged ```python block that
  // happens to define a variable named `pie` must not be hijacked.
  const hasLangClass = /\blanguage-\S+/.test(code.className || '');
  if (hasLangClass) return false;
  return MERMAID_KEYWORDS.test(code.textContent || '');
}

/**
 * Renders ```mermaid fenced code blocks inside `ref`'s injected markdown HTML
 * as inline SVG diagrams. Blocks that fail to parse (e.g. still streaming)
 * are left as plain code and retried on the next content change.
 */
export function useMermaidBlocks(ref, deps) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const blocks = Array.from(el.querySelectorAll('pre > code')).filter(isMermaidBlock);
    if (blocks.length === 0) return undefined;
    let cancelled = false;
    (async () => {
      let mermaid;
      try {
        mermaid = await loadMermaid();
      } catch (e) {
        console.error('[mermaid] failed to load library', e);
        return;
      }
      for (const code of blocks) {
        if (cancelled) return;
        const pre = code.parentElement;
        if (!pre || pre.dataset.mermaidDone === '1') continue;
        const source = code.textContent || '';
        if (!source.trim()) continue;
        const id = `mermaid-chat-${++renderSeq}`;
        try {
          const { svg } = await mermaid.render(id, source);
          if (cancelled || !pre.isConnected) return;
          const wrap = document.createElement('div');
          wrap.className = 'mermaid-diagram relative my-2 flex justify-center overflow-x-auto rounded-md bg-muted/40 p-2';
          wrap.innerHTML = svg;
          fixLabelContrast(wrap);
          const fsBtn = document.createElement('button');
          fsBtn.type = 'button';
          fsBtn.className = 'rustic-copy-btn mermaid-fs-btn';
          fsBtn.setAttribute('aria-label', 'View diagram fullscreen');
          fsBtn.title = 'Fullscreen';
          fsBtn.innerHTML = MAXIMIZE_ICON;
          const svgHtml = wrap.querySelector('svg')?.outerHTML || svg;
          fsBtn.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            openMermaidFullscreen(svgHtml);
          });
          wrap.appendChild(fsBtn);
          pre.replaceWith(wrap);
        } catch (e) {
          // Parse failure (often a partially-streamed block) — keep the code
          // fence visible; mermaid.render leaves a stray error element behind,
          // remove it so it doesn't pile up at the document bottom. Logged so
          // a legitimately broken diagram is diagnosable from the console.
          console.warn('[mermaid] render failed, leaving code fence as-is:', e?.message || e);
          document.getElementById(id)?.remove();
          document.querySelector(`#d${id}`)?.remove();
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
