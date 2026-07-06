import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { visualizer } from 'rollup-plugin-visualizer';
import path from 'node:path';
import { readFileSync } from 'node:fs';

// Single source of truth for the displayed app version: package.json. Injected
// as `__APP_VERSION__` so the web build's `getVersion()` shim (src/lib/web/
// api-app.js) reports the real version instead of a hand-maintained constant
// that drifted every release. The desktop build reads its version from Tauri
// directly and ignores this.
const APP_VERSION = JSON.parse(
  readFileSync(path.resolve(__dirname, 'package.json'), 'utf-8'),
).version;

// The build target selects the transport: 'tauri' (default, desktop,
// unchanged) or 'web' (browser build served by rustic-server). Selected via
// Vite's `--mode web` (portable across OSes) or the VITE_TARGET env var. In the
// web build we alias the Tauri API/plugin entry points to the HTTP+WebSocket
// shims under src/lib/web, so NOT A SINGLE component import has to change —
// `invoke`/`listen` keep their signatures and just route over fetch + /ws
// instead of the Tauri IPC bridge.
function buildWebAliases(isWeb) {
  if (!isWeb) return {};
  return {
    '@tauri-apps/api/core': path.resolve(__dirname, './src/lib/web/core.js'),
    '@tauri-apps/api/event': path.resolve(__dirname, './src/lib/web/event.js'),
    '@tauri-apps/api/window': path.resolve(__dirname, './src/lib/web/api-window.js'),
    '@tauri-apps/api/app': path.resolve(__dirname, './src/lib/web/api-app.js'),
    '@tauri-apps/plugin-shell': path.resolve(__dirname, './src/lib/web/plugin-shell.js'),
    '@tauri-apps/plugin-dialog': path.resolve(__dirname, './src/lib/web/plugin-dialog.js'),
    '@tauri-apps/plugin-fs': path.resolve(__dirname, './src/lib/web/plugin-fs.js'),
    '@tauri-apps/plugin-updater': path.resolve(__dirname, './src/lib/web/plugin-updater.js'),
    '@tauri-apps/plugin-process': path.resolve(__dirname, './src/lib/web/plugin-process.js'),
  };
}

export default defineConfig(({ mode }) => {
  const isWeb = mode === 'web' || process.env.VITE_TARGET === 'web';
  return {
  root: 'src',
  clearScreen: false,
  plugins: [
    react(),
    tailwindcss(),
    // Bundle-size treemap, opt-in only: `ANALYZE=1 bun run build` writes
    // stats.html at the repo root. Normal builds are unaffected.
    ...(process.env.ANALYZE
      ? [
          visualizer({
            filename: path.resolve(__dirname, 'stats.html'),
            gzipSize: true,
            brotliSize: true,
          }),
        ]
      : []),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
      ...buildWebAliases(isWeb),
    },
  },
  server: { port: 1420, strictPort: true },
  define: {
    __IS_WEB__: JSON.stringify(isWeb),
    __APP_VERSION__: JSON.stringify(APP_VERSION),
  },
  envPrefix: ['VITE_', 'TAURI_'],
  // Pre-bundle the editor packages that get pulled in via dynamic
  // imports (Univer for .xlsx, eigenpal for .docx). Without an explicit
  // include list, Vite discovers their deep transitive deps lazily as
  // the user opens a file, which forces a re-optimize mid-request and
  // makes in-flight modules return `504 (Outdated Optimize Dep)`.
  // Listing them here means Vite builds the optimized bundle at server
  // start, so the first file-open is a clean cache hit.
  optimizeDeps: {
    include: [
      '@univerjs/presets',
      '@univerjs/preset-sheets-core',
      '@eigenpal/docx-editor-react',
      'prosemirror-commands',
      'prosemirror-dropcursor',
      'prosemirror-history',
      'prosemirror-keymap',
      'prosemirror-model',
      'prosemirror-state',
      'prosemirror-tables',
      'prosemirror-transform',
      'prosemirror-view',
      'exceljs',
    ],
  },
  build: {
    target: 'esnext',
    outDir: '../dist',
    // `outDir` sits outside the vite `root` ('src'), so vite will NOT empty it
    // automatically and every build otherwise piles new hashed assets on top of
    // the old ones — dist had grown to 585 MB / 2000+ stale chunks, bloating the
    // Tauri installer. Force a clean output dir on every build.
    emptyOutDir: true,
    // `ignoreDynamicRequires: true` tells @rollup/plugin-commonjs to
    // leave dynamic `require(...)` calls in place rather than rewriting
    // them to its `commonjsRequire` helper. Several UMD bundles in
    // vendor only reference `require` behind a `typeof require ===
    // 'function'` guard (which is `false` in browsers anyway), so they
    // never actually call it. Without this option, Rollup pulled the
    // helper into the `xlsx` chunk and made vendor statically import
    // from xlsx, which then statically imported the preload helper from
    // monaco — closing a vendor → xlsx → monaco → vendor cycle that
    // crashed module init in production builds (`Pee.create` /
    // `Rt.memo` on undefined). Dev mode escapes this because Vite
    // pre-bundles deps with esbuild and skips manualChunks entirely.
    commonjsOptions: {
      ignoreDynamicRequires: true,
    },
    // We previously had a `manualChunks` function that split node_modules
    // into named chunks (vendor, codemirror, monaco, radix, docx, xlsx,
    // markdown, …). It produced multiple static-import cycles in prod
    // builds — e.g. vendor → codemirror → docx → radix → vendor — because
    // small slivers of tightly-coupled package families leaked across the
    // named-chunk boundaries. ESM evaluates cycles eagerly, so whichever
    // chunk ran top-level code first (React.memo, forwardRef,
    // state.create, …) saw `undefined` for its cross-chunk imports.
    //
    // Dev mode never hit this because Vite serves modules individually
    // via esbuild and ignores `manualChunks` entirely.
    //
    // Rollup's default chunking (no `manualChunks`) is cycle-free by
    // construction: it groups modules by reachability from each entry +
    // dynamic-import boundary. The resulting chunks are unnamed (hashed),
    // but that's a tolerable cost compared to a black screen on load.
  },
  };
});
