; vendored from nvim-treesitter @ master (2025-05)
; source: https://github.com/nvim-treesitter/nvim-treesitter/blob/master/queries/html/tags.scm
; license: Apache-2.0 / MIT (dual)
;
; HTML doesn't have a meaningful symbol surface the way code languages
; do. Upstream nvim-treesitter doesn't ship a tags.scm for HTML — we
; treat element-id attributes as the closest analogue to a named
; symbol (`<h1 id="intro">` → "intro"). Anchor-link navigation,
; toc generation, and similar use cases all key on id attributes.

(element
  (start_tag
    (attribute
      (attribute_name) @attr_name
      (quoted_attribute_value (attribute_value) @name.variable)))
  (#eq? @attr_name "id"))
