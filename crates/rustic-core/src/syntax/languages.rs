use tree_sitter::Language;

/// Maps file extensions/language names to tree-sitter Language objects.
pub struct LanguageRegistry;

impl LanguageRegistry {
    pub fn get_language(name: &str) -> Option<Language> {
        match name {
            #[cfg(feature = "lang-rust")]
            "rust" => Some(tree_sitter_rust::LANGUAGE.into()),

            #[cfg(feature = "lang-javascript")]
            "javascript" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),

            #[cfg(feature = "lang-typescript")]
            "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),

            #[cfg(feature = "lang-typescript")]
            "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),

            #[cfg(feature = "lang-python")]
            "python" => Some(tree_sitter_python::LANGUAGE.into()),

            #[cfg(feature = "lang-go")]
            "go" => Some(tree_sitter_go::LANGUAGE.into()),

            #[cfg(feature = "lang-c")]
            "c" => Some(tree_sitter_c::LANGUAGE.into()),

            #[cfg(feature = "lang-cpp")]
            "cpp" => Some(tree_sitter_cpp::LANGUAGE.into()),

            #[cfg(feature = "lang-java")]
            "java" => Some(tree_sitter_java::LANGUAGE.into()),

            #[cfg(feature = "lang-json")]
            "json" => Some(tree_sitter_json::LANGUAGE.into()),

            #[cfg(feature = "lang-toml")]
            "toml" => Some(tree_sitter_toml_ng::LANGUAGE.into()),

            #[cfg(feature = "lang-html")]
            "html" => Some(tree_sitter_html::LANGUAGE.into()),

            #[cfg(feature = "lang-css")]
            "css" => Some(tree_sitter_css::LANGUAGE.into()),

            #[cfg(feature = "lang-markdown")]
            "markdown" => Some(tree_sitter_md::LANGUAGE.into()),

            _ => None,
        }
    }

    pub fn get_highlight_query(name: &str) -> Option<&'static str> {
        match name {
            #[cfg(feature = "lang-rust")]
            "rust" => Some(tree_sitter_rust::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-javascript")]
            "javascript" | "jsx" => Some(tree_sitter_javascript::HIGHLIGHT_QUERY),

            #[cfg(feature = "lang-typescript")]
            "typescript" | "tsx" => Some(tree_sitter_typescript::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-python")]
            "python" => Some(tree_sitter_python::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-go")]
            "go" => Some(tree_sitter_go::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-c")]
            "c" => Some(tree_sitter_c::HIGHLIGHT_QUERY),

            #[cfg(feature = "lang-cpp")]
            "cpp" => Some(tree_sitter_cpp::HIGHLIGHT_QUERY),

            #[cfg(feature = "lang-java")]
            "java" => Some(tree_sitter_java::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-json")]
            "json" => Some(tree_sitter_json::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-toml")]
            "toml" => Some(tree_sitter_toml_ng::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-html")]
            "html" => Some(tree_sitter_html::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-css")]
            "css" => Some(tree_sitter_css::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-markdown")]
            "markdown" => Some(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),

            _ => None,
        }
    }
}
