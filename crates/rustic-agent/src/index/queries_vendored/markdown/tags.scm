; vendored from nvim-treesitter @ master (2025-05)
; source: https://github.com/nvim-treesitter/nvim-treesitter/blob/master/queries/markdown/tags.scm
; license: Apache-2.0 / MIT (dual)
;
; Headings as named sections — closest concept the symbol index has
; for "named anchor in a doc". Captured under `@name.module` since
; SymbolKind has no `Heading` variant; the builder treats it as a
; first-class navigable name regardless.
;
; CURRENT STATE: this file is registered in `queries.rs::MARKDOWN`
; but `tree-sitter-markdown` ships ABI 15 against our tree-sitter
; 0.24's max ABI 14, so Query::new fails at runtime. The builder
; logs + skips it cleanly (see `index_one`'s grammar-compat
; fallback). Unblocks when tree-sitter is bumped to 0.25+.

(atx_heading (inline) @name.module)
(setext_heading (paragraph) @name.module)
