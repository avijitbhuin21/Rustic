import { el } from '../../utils/dom.js';
import { updateSetting } from '../../state/settings.js';
import { createCollapsible, createToggleSetting } from './settings-controls.js';

// Languages the backend knows how to start a server for. Must match
// `LspServerConfig::defaults()` in crates/rustic-core/src/lsp/manager.rs.
// `command` is shown as a hint so users know what binary needs to be on PATH.
const LSP_LANGUAGES = [
  { id: 'rust',       label: 'Rust',         command: 'rust-analyzer' },
  { id: 'typescript', label: 'TypeScript / JavaScript', command: 'typescript-language-server' },
  { id: 'python',     label: 'Python',       command: 'pylsp' },
  { id: 'go',         label: 'Go',           command: 'gopls' },
  { id: 'c',          label: 'C / C++',      command: 'clangd' },
  { id: 'json',       label: 'JSON',         command: 'vscode-json-language-server' },
  { id: 'css',        label: 'CSS / SCSS / Less', command: 'vscode-css-language-server' },
  { id: 'html',       label: 'HTML',         command: 'vscode-html-language-server' },
];

export function createLspSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  const lsp = settings.lsp || {};
  const languages = lsp.languages || {};
  const masterEnabled = lsp.enabled !== false;

  // --- Master toggle ---
  const masterContent = el('div', { class: 'settings-collapsible-content' });

  masterContent.appendChild(createToggleSetting(
    'Enable Language Servers',
    'Master switch. When off, no LSP servers are started for any file. ' +
    'Syntax highlighting (tree-sitter) still works, but autocomplete, hover ' +
    'tooltips, go-to-definition, and diagnostics are disabled. Useful on ' +
    'low-memory machines — language servers like rust-analyzer can use ' +
    'hundreds of MB of RAM per project.',
    masterEnabled,
    (v) => updateSetting('lsp.enabled', v),
  ));

  container.appendChild(createCollapsible('Language Servers', masterContent, true));

  // --- Per-language toggles ---
  // Only show the per-language section when the master toggle is ON. With
  // LSP globally disabled, none of these toggles can do anything, so hiding
  // them keeps the panel focused on the one switch that actually matters.
  // Toggling the master switch triggers a settings store update which
  // re-renders this panel, so the section appears/disappears immediately.
  if (masterEnabled) {
    const langContent = el('div', { class: 'settings-collapsible-content' });

    const note = el('div', { class: 'settings-row__desc', style: 'padding: 8px 0 12px 0;' },
      'Toggle individual language servers. Disabled languages keep syntax ' +
      'highlighting but skip semantic features. The server binary must be ' +
      'installed on your PATH for the toggle to do anything.');
    langContent.appendChild(note);

    for (const { id, label, command } of LSP_LANGUAGES) {
      // Default to ON when no explicit setting exists for this language.
      const enabled = languages[id] !== false;
      langContent.appendChild(createToggleSetting(
        label,
        `Server: ${command}`,
        enabled,
        (v) => updateSetting(`lsp.languages.${id}`, v),
      ));
    }

    container.appendChild(createCollapsible('Per-Language Toggles', langContent, true));
  }

  return container;
}
