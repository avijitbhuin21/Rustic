; vendored from nvim-treesitter @ master (2025-05)
; source: https://github.com/nvim-treesitter/nvim-treesitter/blob/master/queries/bash/tags.scm
; license: Apache-2.0 / MIT (dual)
;
; Pinned-commit handling: the upstream file is short enough that we
; carry a syntactic equivalent here directly. When updating, drop in
; the upstream `tags.scm` verbatim and add the new commit SHA above.
;
; Capture-name convention matches our `kind_from_capture` mapper —
; `@name.function` etc. Anything captured under a name we don't
; recognise is silently ignored by the builder.

(function_definition name: (word) @name.function)
(variable_assignment name: (variable_name) @name.constant)
