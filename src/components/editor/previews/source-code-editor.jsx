import React, { useEffect, useMemo, useRef } from 'react';
import CodeMirror from '@uiw/react-codemirror';
import { keymap, EditorView } from '@codemirror/view';
import { oneDark } from '@codemirror/theme-one-dark';

// Resolve the language-specific extension on demand. Most preview surfaces
// only need one language each, so we lazy-load them per call to keep the
// initial preview bundle small. `lang` is the same string @codemirror/* uses
// (e.g. 'markdown', 'html').
async function loadLanguageExtension(lang) {
  switch (lang) {
    case 'markdown': {
      const m = await import('@codemirror/lang-markdown');
      return m.markdown({ codeLanguages: [] });
    }
    case 'html':
    case 'xml':
    case 'svg': {
      // lang-html parses XML-ish content well enough for our svg/xml uses;
      // we don't ship a separate lang-xml so we route both through it.
      const m = await import('@codemirror/lang-html');
      return m.html();
    }
    case 'css': {
      const m = await import('@codemirror/lang-css');
      return m.css();
    }
    case 'json': {
      const m = await import('@codemirror/lang-json');
      return m.json();
    }
    default:
      return null;
  }
}

// Source editor backed by CodeMirror with the oneDark theme. Used in every
// preview that has an Edit tab (markdown, html, svg). We deliberately keep
// the surface area narrow — value/onChange/onSave/lang — and bury CodeMirror
// setup behind it so the previews don't all repeat the language-loading
// dance.
export function SourceCodeEditor({ value, onChange, onSave, lang }) {
  // Languages are dynamic; we feed them through state via a ref + force
  // re-render to avoid an empty initial flash while the language is loading.
  const [extensions, setExtensions] = React.useState(() => baseExtensions(onSave));
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const langExt = lang ? await loadLanguageExtension(lang) : null;
      if (cancelled) return;
      const base = baseExtensions(() => onSaveRef.current?.());
      setExtensions(langExt ? [...base, langExt] : base);
    })();
    return () => {
      cancelled = true;
    };
  }, [lang]);

  // Memoise basicSetup so we don't pass a new object identity on every
  // render — CodeMirror tears down + rebuilds the editor when the
  // `basicSetup` reference changes, which would lose the cursor and
  // selection on every keystroke.
  const basicSetup = useMemo(
    () => ({
      lineNumbers: true,
      highlightActiveLine: true,
      highlightActiveLineGutter: true,
      foldGutter: true,
      autocompletion: false,
      // Editor handles Tab inside <kbd>Tab</kbd>-aware contexts; for the
      // top-level preview surface we just want indent.
      indentOnInput: true,
    }),
    [],
  );

  return (
    <CodeMirror
      value={value}
      onChange={onChange}
      theme={oneDark}
      basicSetup={basicSetup}
      extensions={extensions}
      height="100%"
      style={{ height: '100%' }}
      // Wrap long lines so reading prose in markdown / HTML doesn't require
      // horizontal scroll. Code blocks still scroll because CodeMirror
      // preserves them as their own scrollable lines.
    />
  );
}

// Extensions shared regardless of language: oneDark theme, soft line wrap,
// and a Ctrl/Cmd+S keymap that hooks into the caller's save function.
function baseExtensions(onSave) {
  return [
    EditorView.lineWrapping,
    keymap.of([
      {
        key: 'Mod-s',
        preventDefault: true,
        run: () => {
          if (typeof onSave === 'function') onSave();
          return true;
        },
      },
    ]),
  ];
}

export default SourceCodeEditor;
