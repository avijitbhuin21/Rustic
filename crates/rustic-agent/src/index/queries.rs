//! Tree-sitter "tags" queries, one per language. Captures top-level declarations
//! into `@name.<kind>`; intentionally conservative to avoid false positives.

use super::symbol::SymbolKind;

/// Map a language name to its tree-sitter tags query source.
pub fn query_source(lang_name: &str) -> Option<&'static str> {
    Some(match lang_name {
        "rust" => RUST,
        "typescript" => TS,
        "tsx" => TSX,
        "javascript" => JS,
        "python" => PYTHON,
        "go" => GO,
        "java" => JAVA,
        "c" => C,
        "cpp" => CPP,
        "ruby" => RUBY,
        "php" => PHP,
        "csharp" => CSHARP,
        "kotlin" => KOTLIN,
        "swift" => SWIFT,
        "scala" => SCALA,
        "bash" => BASH,
        "markdown" => MARKDOWN,
        "html" => HTML,
        "css" => CSS,
        _ => return None,
    })
}

/// Map a query capture name (e.g. `name.function`) to the kind we record.
/// Capture names that don't start with `name.` are ignored.
pub fn kind_from_capture(capture_name: &str) -> Option<SymbolKind> {
    let suffix = capture_name.strip_prefix("name.")?;
    Some(match suffix {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "interface" => SymbolKind::Interface,
        "type" => SymbolKind::TypeAlias,
        "module" => SymbolKind::Module,
        "variable" => SymbolKind::Variable,
        "constant" => SymbolKind::Constant,
        "macro" => SymbolKind::Macro,
        _ => return None,
    })
}

const RUST: &str = r#"
(function_item name: (identifier) @name.function)
(struct_item name: (type_identifier) @name.struct)
(enum_item name: (type_identifier) @name.enum)
(union_item name: (type_identifier) @name.struct)
(trait_item name: (type_identifier) @name.trait)
(type_item name: (type_identifier) @name.type)
(const_item name: (identifier) @name.constant)
(static_item name: (identifier) @name.constant)
(mod_item name: (identifier) @name.module)
(macro_definition name: (identifier) @name.macro)
(impl_item
  body: (declaration_list
    (function_item name: (identifier) @name.method)))
(trait_item
  body: (declaration_list
    (function_item name: (identifier) @name.method)))
"#;

const TS: &str = r#"
(function_declaration name: (identifier) @name.function)
(class_declaration name: (type_identifier) @name.class)
(interface_declaration name: (type_identifier) @name.interface)
(type_alias_declaration name: (type_identifier) @name.type)
(enum_declaration name: (identifier) @name.enum)
(method_definition name: (property_identifier) @name.method)
(public_field_definition
  name: (property_identifier) @name.variable
  value: (arrow_function))
(lexical_declaration
  (variable_declarator
    name: (identifier) @name.constant
    value: [(arrow_function) (function_expression)]))
"#;

const TSX: &str = TS;

const JS: &str = r#"
(function_declaration name: (identifier) @name.function)
(class_declaration name: (identifier) @name.class)
(method_definition name: (property_identifier) @name.method)
(lexical_declaration
  (variable_declarator
    name: (identifier) @name.constant
    value: [(arrow_function) (function_expression)]))
"#;

const PYTHON: &str = r#"
(function_definition name: (identifier) @name.function)
(class_definition name: (identifier) @name.class)
(class_definition
  body: (block
    (function_definition name: (identifier) @name.method)))
(decorated_definition
  definition: (function_definition name: (identifier) @name.function))
(decorated_definition
  definition: (class_definition name: (identifier) @name.class))
"#;

const GO: &str = r#"
(function_declaration name: (identifier) @name.function)
(method_declaration name: (field_identifier) @name.method)
(type_spec name: (type_identifier) @name.type)
(const_spec name: (identifier) @name.constant)
"#;

const JAVA: &str = r#"
(class_declaration name: (identifier) @name.class)
(interface_declaration name: (identifier) @name.interface)
(enum_declaration name: (identifier) @name.enum)
(method_declaration name: (identifier) @name.method)
(constructor_declaration name: (identifier) @name.method)
"#;

const C: &str = r#"
(function_definition declarator: (function_declarator declarator: (identifier) @name.function))
(struct_specifier name: (type_identifier) @name.struct)
(enum_specifier name: (type_identifier) @name.enum)
(type_definition declarator: (type_identifier) @name.type)
"#;

const CPP: &str = r#"
(function_definition declarator: (function_declarator declarator: (identifier) @name.function))
(function_definition declarator: (function_declarator declarator: (field_identifier) @name.method))
(function_definition declarator: (function_declarator declarator: (qualified_identifier) @name.method))
(class_specifier name: (type_identifier) @name.class)
(struct_specifier name: (type_identifier) @name.struct)
(enum_specifier name: (type_identifier) @name.enum)
"#;

const RUBY: &str = r#"
(method name: (identifier) @name.method)
(singleton_method name: (identifier) @name.method)
(class name: (constant) @name.class)
(module name: (constant) @name.module)
"#;

const PHP: &str = r#"
(function_definition name: (name) @name.function)
(method_declaration name: (name) @name.method)
(class_declaration name: (name) @name.class)
(interface_declaration name: (name) @name.interface)
(trait_declaration name: (name) @name.trait)
"#;

const CSHARP: &str = r#"
(class_declaration name: (identifier) @name.class)
(interface_declaration name: (identifier) @name.interface)
(struct_declaration name: (identifier) @name.struct)
(enum_declaration name: (identifier) @name.enum)
(method_declaration name: (identifier) @name.method)
(constructor_declaration name: (identifier) @name.method)
"#;

const KOTLIN: &str = r#"
(function_declaration (simple_identifier) @name.function)
(class_declaration (type_identifier) @name.class)
(object_declaration (type_identifier) @name.class)
(property_declaration
  (variable_declaration (simple_identifier) @name.constant))
"#;

const SWIFT: &str = r#"
(function_declaration name: (simple_identifier) @name.function)
(class_declaration name: (type_identifier) @name.class)
(protocol_declaration name: (type_identifier) @name.interface)
"#;

const SCALA: &str = r#"
(class_definition name: (identifier) @name.class)
(trait_definition name: (identifier) @name.trait)
(object_definition name: (identifier) @name.class)
(function_definition name: (identifier) @name.function)
(function_declaration name: (identifier) @name.function)
"#;

// Declarative/scripting grammars read from vendored queries_vendored/<lang>/tags.scm.
// See VENDORED.md for the swap protocol when upgrading.
const BASH: &str = include_str!("queries_vendored/bash/tags.scm");
const MARKDOWN: &str = include_str!("queries_vendored/markdown/tags.scm");
const HTML: &str = include_str!("queries_vendored/html/tags.scm");
const CSS: &str = include_str!("queries_vendored/css/tags.scm");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_languages_have_queries() {
        for lang in [
            "rust",
            "typescript",
            "tsx",
            "javascript",
            "python",
            "go",
            "java",
            "c",
            "cpp",
            "ruby",
            "php",
            "csharp",
            "kotlin",
            "swift",
            "scala",
            "bash",
            "markdown",
            "html",
            "css",
        ] {
            assert!(
                query_source(lang).is_some(),
                "missing query source for `{}`",
                lang
            );
        }
    }

    fn compile_for(lang_name: &str) -> std::result::Result<(), String> {
        let lang = rustic_treesitter::LanguageRegistry::get_language(lang_name)
            .ok_or_else(|| format!("no grammar registered for `{}`", lang_name))?;
        let src = query_source(lang_name)
            .ok_or_else(|| format!("no query source for `{}`", lang_name))?;
        tree_sitter::Query::new(&lang, src).map(|_| ()).map_err(|e| e.to_string())
    }

    #[test]
    fn bash_query_compiles() {
        compile_for("bash").expect("bash tags query must compile");
    }

    #[test]
    fn markdown_query_compiles() {
        compile_for("markdown").expect("markdown tags query must compile");
    }

    #[test]
    fn html_query_compiles() {
        compile_for("html").expect("html tags query must compile");
    }

    #[test]
    fn css_query_compiles() {
        compile_for("css").expect("css tags query must compile");
    }

    #[test]
    fn unknown_language_returns_none() {
        assert!(query_source("brainfuck").is_none());
    }

    fn refresh_one_file(name: &str, body: &str) -> Vec<crate::index::SymbolEntry> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        let ts = std::sync::Arc::new(rustic_treesitter::WorkspaceTreesitter::new());
        let idx = std::sync::Arc::new(crate::index::SymbolIndex::new());
        let _ = crate::index::refresh_file(&path, &ts, &idx);
        idx.entries_in_file(&path)
    }

    #[test]
    fn bash_function_definition_yields_symbol() {
        let entries = refresh_one_file(
            "script.sh",
            "#!/bin/bash\nfunction do_thing() {\n  echo hi\n}\n",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"do_thing"),
            "expected `do_thing` in bash symbols, got {:?}",
            names
        );
    }

    #[test]
    fn css_class_and_id_selectors_yield_symbols() {
        let entries = refresh_one_file(
            "style.css",
            ".primary-btn { color: red; }\n#sidebar { width: 200px; }\n",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"primary-btn"),
            "expected class selector, got {:?}",
            names,
        );
        assert!(
            names.contains(&"sidebar"),
            "expected id selector, got {:?}",
            names,
        );
    }

    #[test]
    fn html_id_attribute_yields_symbol() {
        let entries = refresh_one_file(
            "page.html",
            "<html><body><h1 id=\"intro\">Hi</h1></body></html>",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"intro"),
            "expected `intro` from id attribute, got {:?}",
            names,
        );
    }

    #[test]
    fn markdown_atx_headings_yield_symbols() {
        let entries = refresh_one_file(
            "doc.md",
            "# Introduction\n\nSome prose.\n\n## API Reference\n\nMore.\n\n### Authentication\n\nDetails.\n",
        );
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        // The inline-text of each heading lands as a module-kind entry.
        // Exact whitespace handling varies across grammar versions, so
        // assert by substring rather than exact equality.
        assert!(
            names.iter().any(|n| n.contains("Introduction")),
            "expected `Introduction` heading, got {:?}",
            names,
        );
        assert!(
            names.iter().any(|n| n.contains("API Reference")),
            "expected `API Reference` heading, got {:?}",
            names,
        );
        assert!(
            names.iter().any(|n| n.contains("Authentication")),
            "expected `Authentication` heading, got {:?}",
            names,
        );
    }

    #[test]
    fn markdown_setext_headings_also_yield_symbols() {
        let entries = refresh_one_file(
            "doc.md",
            "Top Heading\n===========\n\nProse.\n\nSecond Heading\n--------------\n\nMore.\n",
        );
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        assert!(
            names.iter().any(|n| n.contains("Top Heading")),
            "expected setext H1, got {:?}",
            names,
        );
        assert!(
            names.iter().any(|n| n.contains("Second Heading")),
            "expected setext H2, got {:?}",
            names,
        );
    }

    #[test]
    fn capture_kind_round_trip() {
        assert_eq!(kind_from_capture("name.function"), Some(SymbolKind::Function));
        assert_eq!(kind_from_capture("name.struct"), Some(SymbolKind::Struct));
        assert_eq!(kind_from_capture("name.module"), Some(SymbolKind::Module));
        assert_eq!(kind_from_capture("unknown"), None);
        assert_eq!(kind_from_capture("name.frobnicator"), None);
    }
}
