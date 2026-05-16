# Tags queries — vendoring contract

The tree-sitter "tags" queries that drive the workspace symbol index
live as `&'static str` constants in `queries.rs`. They are **hand-rolled
and intentionally minimal**: every captured node maps cleanly to one of
the kinds in `SymbolKind`, and we err on the side of under-capturing so
`find_symbol` lookups don't drown in false positives.

For richer coverage (e.g. capturing every Python `lambda`, every Rust
`use` statement as a "reference", or HTML `data-*` attributes as
anchors), the recommended upstream is
[nvim-treesitter/nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter)
— their `queries/<lang>/tags.scm` files are MIT/Apache-2.0 licensed and
are kept current against the upstream grammar releases.

## Swap protocol

When swapping a hand-rolled constant for a vendored upstream:

1. Add a sub-directory `crates/rustic-agent/src/index/queries_vendored/<lang>/`
   and drop `tags.scm` there.
2. Add a one-line provenance comment at the top of the `.scm` file:
   ```
   ; vendored from nvim-treesitter @ <commit-sha>
   ; source: https://github.com/nvim-treesitter/nvim-treesitter/blob/<sha>/queries/<lang>/tags.scm
   ; license: Apache-2.0 + MIT (dual)
   ```
3. Replace the `const FOO: &str = r#"..."#;` with
   `const FOO: &str = include_str!("queries_vendored/<lang>/tags.scm");`.
4. Update the capture-name → `SymbolKind` mapping in
   `kind_from_capture` if the vendored query uses capture names this
   crate doesn't currently recognise (most upstream queries follow the
   same `@name.function` / `@name.method` convention).
5. Update the test in `queries.rs` (the `*_query_compiles` tests use
   `tree_sitter::Query::new` to verify the source compiles against the
   live grammar — they'll catch grammar/ABI drift before runtime).
6. Pin the upstream commit in this file under the entry below.

## Current vendored sources

(None yet. C2 lays the contract; the actual vendoring is a follow-up
task that depends on `tree-sitter` and grammar versions being aligned
across rustic-core and rustic-agent.)

## Hand-rolled coverage

| Language | Query scope | Replace with nvim-treesitter? |
|---|---|---|
| rust | items + impl/trait methods | Optional — covers ~95% of useful symbols |
| typescript / tsx / javascript | functions / classes / interfaces | Optional |
| python | functions / classes / module-constants | Optional |
| go | functions / methods / types | Optional |
| java / c / cpp / ruby / php / csharp / kotlin / swift / scala | basic top-level decls | Recommended (upstream much richer) |
| bash | `function_definition` only | Recommended once we care about variable assignments |
| markdown | ATX + setext headings as modules | Blocked: tree-sitter-markdown ABI mismatch (#TODO realign) |
| html | `id="..."` anchors only | Recommended when we want `data-*` / class anchors |
| css | class + id selectors | Optional |

## Note on the markdown grammar

**Resolved 2026-05-14.** `tree-sitter` was bumped from 0.24 → 0.26 across
all crates; ABI 15 (and 16) is now supported. `MARKDOWN` query compiles
against the live grammar and `find_symbol` / `outline` / `call_sites`
now operate on `.md` files. ATX (`# H1`) and setext (`---` underline)
headings both extract as `SymbolKind::Module` entries.

Historical note (kept for posterity): prior to the bump, the indexer's
grammar-compat fallback in `index_one` swallowed the compile error and
returned 0 symbols for markdown — the index kept working on the other
18 languages, just without markdown coverage.
