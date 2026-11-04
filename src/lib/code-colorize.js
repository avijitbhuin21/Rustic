// Editor-identical syntax colouring for read-only previews (e.g. the Cloud &
// Sync remote file pane): Monaco's `colorize` with the app's live theme, so
// tokens match what the same file looks like in an editor tab.
import { loader } from '@monaco-editor/react';
import { watchMonacoTheme } from '@/lib/monaco-theme';

// Above this, colouring a preview costs more than it helps; show plain text.
export const COLORIZE_MAX_CHARS = 256 * 1024;

let monacoReady = null;

/** Monaco instance, initialised once and kept in sync with the app theme. */
export function getMonaco() {
  if (!monacoReady) {
    monacoReady = import('monaco-editor')
      .then((monaco) => {
        loader.config({ monaco });
        return loader.init();
      })
      .then((monaco) => {
        watchMonacoTheme(monaco);
        return monaco;
      })
      .catch((e) => {
        monacoReady = null;
        throw e;
      });
  }
  return monacoReady;
}

const warmed = new Map();

/**
 * Make sure `language` has a tokenizer. `colorize` only triggers Monaco's basic
 * (Monarch) languages; JSON's tokenizer lives in its language mode, which loads
 * only when a model of that language is created — so create a throwaway one and
 * wait (briefly) for the mode to register.
 */
function ensureTokenizer(monaco, language, sample) {
  if (!warmed.has(language)) {
    warmed.set(language, (async () => {
      const hasTokens = () => monaco.editor
        .tokenize(sample, language)
        .some((line) => line.some((t) => t.type && t.type !== language));
      if (hasTokens()) return;
      monaco.editor.createModel('', language).dispose();
      for (let i = 0; i < 40 && !hasTokens(); i += 1) {
        await new Promise((r) => setTimeout(r, 50));
      }
    })());
  }
  return warmed.get(language);
}

/** Highlighted HTML for `text` in Monaco `language`, or null when it shouldn't be coloured. */
export async function colorizeToHtml(text, language) {
  if (!text || !language || language === 'plaintext' || text.length > COLORIZE_MAX_CHARS) return null;
  const monaco = await getMonaco();
  await ensureTokenizer(monaco, language, text.slice(0, 400));
  return monaco.editor.colorize(text, language, { tabSize: 2 });
}
