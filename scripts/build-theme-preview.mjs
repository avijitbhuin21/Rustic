// Builds the live theme preview (src/theme-preview.html) into the website, so
// the Theme Park can embed it. The output is committed: the website itself has
// no build step. Run after changing anything the preview shows:
//   bun run build:theme-preview
import { build } from 'vite';
import { copyFileSync, renameSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { MOCK_FILE_NAMES } from '../src/theme-preview/mock-data.js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const park = path.join(root, 'website/static/theme-park');
const outDir = path.join(park, 'preview');

/** Rewrite the icons of the mock's file names (the full icon set is ~500 KB). */
function writeFileIcons() {
  const { getIcon } = createRequire(import.meta.url)('material-file-icons');
  const icons = Object.fromEntries(MOCK_FILE_NAMES.map((name) => [name, getIcon(name).svg]));
  writeFileSync(path.join(root, 'src/theme-preview/file-icons.json'), `${JSON.stringify(icons, null, 2)}\n`);
}

writeFileIcons();
await build({
  configFile: path.join(root, 'vite.config.js'),
  mode: 'web',
  // Relative URLs: the bundle is served from a sub-folder of the website.
  base: './',
  logLevel: 'warn',
  build: {
    outDir,
    emptyOutDir: true,
    rollupOptions: { input: path.join(root, 'src/theme-preview.html') },
  },
});
renameSync(path.join(outDir, 'theme-preview.html'), path.join(outDir, 'index.html'));
copyFileSync(path.join(root, 'src/theme-preview/host.js'), path.join(park, 'preview-host.js'));
console.log(`theme preview -> ${path.relative(root, outDir)}`);
