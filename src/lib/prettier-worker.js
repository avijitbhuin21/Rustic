// Web Worker that runs Prettier in isolation from the renderer thread.
//
// Lives in a separate file so Vite gives it its own chunk — none of the
// ~4 MB Prettier bundle is parsed until the worker is first spawned (on the
// first format-on-save for a Prettier-handled language). After that the
// worker stays warm for subsequent saves until the client decides to kill it.
//
// Plugins are imported eagerly inside the worker because the import latency
// would otherwise be paid on every format call. They're cheap once the worker
// is alive.

import * as prettier from 'prettier/standalone';
import babelPlugin from 'prettier/plugins/babel';
import estreePlugin from 'prettier/plugins/estree';
import typescriptPlugin from 'prettier/plugins/typescript';
import postcssPlugin from 'prettier/plugins/postcss';
import htmlPlugin from 'prettier/plugins/html';
import markdownPlugin from 'prettier/plugins/markdown';
import yamlPlugin from 'prettier/plugins/yaml';

const PLUGINS = [
  babelPlugin,
  estreePlugin,
  typescriptPlugin,
  postcssPlugin,
  htmlPlugin,
  markdownPlugin,
  yamlPlugin,
];

// Map Monaco language ids to Prettier parser names. Anything not in this map
// is rejected by the worker so the caller can fall through to other formatters.
const PARSER_BY_LANGUAGE = {
  javascript: 'babel',
  javascriptreact: 'babel',
  jsx: 'babel',
  typescript: 'typescript',
  typescriptreact: 'typescript',
  tsx: 'typescript',
  json: 'json',
  jsonc: 'json',
  json5: 'json5',
  css: 'css',
  scss: 'scss',
  less: 'less',
  html: 'html',
  vue: 'vue',
  angular: 'angular',
  markdown: 'markdown',
  mdx: 'mdx',
  yaml: 'yaml',
};

self.onmessage = async (ev) => {
  const { id, source, language, options } = ev.data || {};
  try {
    const parser = PARSER_BY_LANGUAGE[language?.toLowerCase()];
    if (!parser) {
      self.postMessage({ id, ok: false, error: `prettier: no parser for language "${language}"` });
      return;
    }
    const formatted = await prettier.format(source, {
      parser,
      plugins: PLUGINS,
      ...(options || {}),
    });
    self.postMessage({ id, ok: true, formatted });
  } catch (err) {
    self.postMessage({ id, ok: false, error: String(err?.message ?? err) });
  }
};
