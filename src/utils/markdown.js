import { el } from './dom.js';

/**
 * Minimal, safe-ish markdown renderer. Handles:
 *   - # / ## / ### headings
 *   - **bold**, *italic*, `inline code`
 *   - fenced ```code blocks```
 *   - blank-line-separated paragraphs
 *   - - / * unordered lists, 1. ordered lists
 *   - > blockquotes
 *
 * No HTML passthrough — everything goes through textContent on leaf nodes.
 */
export function renderMarkdown(src) {
  const container = el('div', { class: 'md' });
  const lines = src.replace(/\r\n/g, '\n').split('\n');

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];

    // Fenced code block
    if (/^```/.test(line)) {
      const lang = line.slice(3).trim();
      const buf = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      if (i < lines.length) i++; // consume closing ```
      const pre = el('pre', { class: 'md-code' + (lang ? ` md-code--${lang}` : '') });
      const code = document.createElement('code');
      code.textContent = buf.join('\n');
      pre.appendChild(code);
      container.appendChild(pre);
      continue;
    }

    // Blank line
    if (/^\s*$/.test(line)) { i++; continue; }

    // Headings
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      const level = h[1].length;
      const head = el(`h${Math.min(level, 4)}`, { class: `md-h md-h${level}` });
      appendInline(head, h[2]);
      container.appendChild(head);
      i++;
      continue;
    }

    // Blockquote (single line, simple)
    if (/^\s*>\s?/.test(line)) {
      const bq = el('blockquote', { class: 'md-bq' });
      const buf = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*>\s?/, ''));
        i++;
      }
      appendInline(bq, buf.join(' '));
      container.appendChild(bq);
      continue;
    }

    // List
    if (/^\s*([-*]|\d+\.)\s+/.test(line)) {
      const ordered = /^\s*\d+\.\s+/.test(line);
      const list = el(ordered ? 'ol' : 'ul', { class: 'md-list' });
      while (i < lines.length && /^\s*([-*]|\d+\.)\s+/.test(lines[i])) {
        const liText = lines[i].replace(/^\s*([-*]|\d+\.)\s+/, '');
        const li = el('li', {});
        appendInline(li, liText);
        list.appendChild(li);
        i++;
      }
      container.appendChild(list);
      continue;
    }

    // Paragraph — consume consecutive non-blank, non-block lines
    const buf = [line];
    i++;
    while (i < lines.length && !/^\s*$/.test(lines[i]) && !/^(#{1,6}\s|```|\s*([-*]|\d+\.)\s|\s*>\s)/.test(lines[i])) {
      buf.push(lines[i]);
      i++;
    }
    const p = el('p', { class: 'md-p' });
    appendInline(p, buf.join(' '));
    container.appendChild(p);
  }

  return container;
}

/**
 * Render inline markdown (bold / italic / code / links) into `node` as
 * safe DOM fragments.
 */
function appendInline(node, text) {
  // Tokenize greedily by matching the next inline marker.
  let remaining = text;
  const pattern = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*]+\*)|(\[[^\]]+\]\([^)]+\))/;
  while (remaining.length > 0) {
    const m = remaining.match(pattern);
    if (!m) {
      node.appendChild(document.createTextNode(remaining));
      return;
    }
    const [match] = m;
    const idx = m.index;
    if (idx > 0) {
      node.appendChild(document.createTextNode(remaining.slice(0, idx)));
    }
    if (match.startsWith('`')) {
      const code = el('code', { class: 'md-inline-code' }, match.slice(1, -1));
      node.appendChild(code);
    } else if (match.startsWith('**')) {
      const strong = el('strong', {}, match.slice(2, -2));
      node.appendChild(strong);
    } else if (match.startsWith('*')) {
      const em = el('em', {}, match.slice(1, -1));
      node.appendChild(em);
    } else if (match.startsWith('[')) {
      const linkMatch = match.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      if (linkMatch) {
        const a = el('a', { class: 'md-link', href: linkMatch[2], target: '_blank', rel: 'noopener noreferrer' }, linkMatch[1]);
        node.appendChild(a);
      } else {
        node.appendChild(document.createTextNode(match));
      }
    }
    remaining = remaining.slice(idx + match.length);
  }
}
