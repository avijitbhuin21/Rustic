import { el } from '../../utils/dom.js';
import { updateSetting } from '../../state/settings.js';
import {
  createCollapsible, createNumberSetting, createToggleSetting,
  createSelectSetting, createTextareaSetting,
} from './settings-controls.js';

export function createEditorSettings(settings) {
  const container = el('div', { class: 'settings-section' });

  const editor = settings.editor || {};

  // --- Tab & Indentation (collapsible) ---
  const tabContent = el('div', { class: 'settings-collapsible-content' });

  tabContent.appendChild(createNumberSetting(
    'Tab Size', 'Number of spaces per tab',
    editor.tab_size ?? 4, 1, 8, 1,
    (v) => updateSetting('editor.tab_size', parseInt(v, 10))
  ));

  tabContent.appendChild(createToggleSetting(
    'Insert Spaces', 'Use spaces instead of tab characters',
    editor.insert_spaces ?? true,
    (v) => updateSetting('editor.insert_spaces', v)
  ));

  container.appendChild(createCollapsible('Tab & Indentation', tabContent, true));

  // --- Display (collapsible) ---
  const displayContent = el('div', { class: 'settings-collapsible-content' });

  displayContent.appendChild(createToggleSetting(
    'Word Wrap', 'Wrap long lines at the viewport edge',
    editor.word_wrap ?? false,
    (v) => updateSetting('editor.word_wrap', v)
  ));

  displayContent.appendChild(createToggleSetting(
    'Line Numbers', 'Show line numbers in the gutter',
    editor.line_numbers ?? true,
    (v) => updateSetting('editor.line_numbers', v)
  ));

  displayContent.appendChild(createToggleSetting(
    'Minimap', 'Show a minimap overview of the file',
    editor.minimap ?? false,
    (v) => updateSetting('editor.minimap', v)
  ));

  displayContent.appendChild(createSelectSetting(
    'Render Whitespace', 'Show whitespace characters',
    editor.render_whitespace ?? 'none',
    ['none', 'boundary', 'all'],
    (v) => updateSetting('editor.render_whitespace', v)
  ));

  displayContent.appendChild(createToggleSetting(
    'Show Zero-Width Characters',
    'Highlight invisible zero-width characters (U+200B, U+200C, U+200D, U+FEFF, etc.)',
    editor.show_zero_width ?? false,
    (v) => updateSetting('editor.show_zero_width', v)
  ));

  displayContent.appendChild(createToggleSetting(
    'Bracket Pair Colorization',
    'Colorize matching bracket pairs for easier code reading',
    editor.bracket_pair_colorization ?? false,
    (v) => updateSetting('editor.bracket_pair_colorization', v)
  ));

  displayContent.appendChild(createToggleSetting(
    'Format on Save',
    'Automatically fix indentation and formatting when saving a file',
    editor.format_on_save ?? true,
    (v) => updateSetting('editor.format_on_save', v)
  ));

  container.appendChild(createCollapsible('Display', displayContent, true));

  // --- Cursor (collapsible) ---
  const cursorContent = el('div', { class: 'settings-collapsible-content' });

  cursorContent.appendChild(createToggleSetting(
    'Cursor Blink', 'Animate the cursor',
    editor.cursor_blink ?? true,
    (v) => updateSetting('editor.cursor_blink', v)
  ));

  cursorContent.appendChild(createSelectSetting(
    'Cursor Style', 'Shape of the text cursor',
    editor.cursor_style ?? 'line',
    [
      { value: 'line', label: 'Line' },
      { value: 'block', label: 'Block' },
      { value: 'underline', label: 'Underline' },
      { value: 'custom-svg', label: 'Custom SVG' },
    ],
    (v) => {
      updateSetting('editor.cursor_style', v);
      // Show/hide SVG input
      svgSection.style.display = v === 'custom-svg' ? '' : 'none';
    }
  ));

  // Custom SVG cursor input
  const svgSection = el('div', {
    class: 'settings-svg-cursor',
    style: (editor.cursor_style === 'custom-svg') ? '' : 'display: none',
  });

  const svgInfo = el('div', { class: 'settings-row__info', style: 'margin-bottom: 8px' });
  svgInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Custom Cursor SVG'));
  svgInfo.appendChild(el('div', { class: 'settings-row__desc' },
    'Paste SVG markup for a custom cursor. It will be scaled to fit the line height (~20px). Keep it simple for best performance.'));
  svgSection.appendChild(svgInfo);

  const svgTextarea = el('textarea', {
    class: 'settings-textarea',
    placeholder: '<svg viewBox="0 0 8 20" xmlns="http://www.w3.org/2000/svg">\n  <rect width="2" height="20" fill="#8ec07c" rx="1"/>\n</svg>',
    rows: '6',
  });
  svgTextarea.value = editor.cursor_custom_svg || '';
  svgSection.appendChild(svgTextarea);

  // SVG preview
  const svgPreview = el('div', { class: 'settings-svg-preview' });
  const updateSvgPreview = () => {
    const svg = svgTextarea.value.trim();
    svgPreview.replaceChildren();
    if (svg && svg.startsWith('<svg')) {
      const previewLabel = el('span', { class: 'settings-row__desc' }, 'Preview: ');
      svgPreview.appendChild(previewLabel);
      const previewEl = el('div', { class: 'settings-svg-preview__cursor' });
      // F-03: sanitise pasted SVG before inserting into the DOM. The settings
      // panel is the user attacking themselves (low impact) but the same
      // helper used for the file-tree preview costs nothing here and keeps a
      // single XSS-resistant insertion path.
      // eslint-disable-next-line import/no-cycle
      import('../../lib/markdown.js').then(({ sanitizeSvg }) => {
        const safe = sanitizeSvg(svg);
        if (safe) {
          previewEl.appendChild(safe);
          if (safe.style) {
            safe.style.height = '20px';
            safe.style.width = 'auto';
          }
        }
      });
      svgPreview.appendChild(previewEl);
    }
  };
  svgTextarea.addEventListener('input', updateSvgPreview);
  updateSvgPreview();
  svgSection.appendChild(svgPreview);

  const saveSvgBtn = el('button', { class: 'settings-btn' }, 'Save Custom Cursor');
  saveSvgBtn.addEventListener('click', () => {
    updateSetting('editor.cursor_custom_svg', svgTextarea.value.trim());
    saveSvgBtn.textContent = 'Saved!';
    setTimeout(() => { saveSvgBtn.textContent = 'Save Custom Cursor'; }, 1500);
  });
  svgSection.appendChild(saveSvgBtn);

  cursorContent.appendChild(svgSection);
  container.appendChild(createCollapsible('Cursor', cursorContent, true));

  return container;
}
