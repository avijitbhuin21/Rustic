; vendored from nvim-treesitter @ master (2025-05)
; source: https://github.com/nvim-treesitter/nvim-treesitter/blob/master/queries/css/tags.scm
; license: Apache-2.0 / MIT (dual)
;
; The upstream nvim-treesitter CSS queries focus on highlight + inject,
; not on tag-style symbol enumeration — there's no official tags.scm
; for CSS in upstream. We carry minimal class/id-selector captures
; that mirror what a tags-style query would emit; revisit when
; nvim-treesitter adds one.

(class_selector (class_name) @name.class)
(id_selector (id_name) @name.variable)
