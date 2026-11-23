# Vendored: Looseleaf ink engine

Source: https://github.com/kevintngo/looseleaf (commit `4bdb647`)
Licence: MIT, covering the code, the art and the Kevin Sans typeface (see `LICENSE` in this folder).

This folder is a subset of upstream `src/lib/ink`: the import closure of the
recipes Rustic draws (buttons, fields, checkboxes, switches, tabs, sheets,
doodles and friends). Charts, stickers, mascots and the paper-object recipes
are not included. Kevin Sans lives in `src/assets/fonts/KevinSans-*.woff2`.

Files are copied verbatim, with one local patch:

- `react/defs.tsx`: `ensureInkFonts()` is a no-op. Upstream injects a remote
  stylesheet from GitHub Pages; Rustic bundles the font and must work offline.

Rustic's own glue lives outside this folder: `src/lib/ink-kit.js` (lazy chunk
and Rustic-specific recipes), `src/components/ui/ink.jsx` and
`src/styles/ink.css`. Keep it that way so an upstream refresh stays a copy.
