use std::sync::LazyLock;
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

            #[cfg(feature = "lang-bash")]
            "bash" => Some(tree_sitter_bash::LANGUAGE.into()),

            #[cfg(feature = "lang-ruby")]
            "ruby" => Some(tree_sitter_ruby::LANGUAGE.into()),

            #[cfg(feature = "lang-php")]
            "php" => Some(tree_sitter_php::LANGUAGE_PHP.into()),

            #[cfg(feature = "lang-yaml")]
            "yaml" => Some(tree_sitter_yaml::LANGUAGE.into()),

            #[cfg(feature = "lang-lua")]
            "lua" => Some(tree_sitter_lua::LANGUAGE.into()),

            #[cfg(feature = "lang-scala")]
            "scala" => Some(tree_sitter_scala::LANGUAGE.into()),

            #[cfg(feature = "lang-swift")]
            "swift" => Some(tree_sitter_swift::LANGUAGE.into()),

            #[cfg(feature = "lang-dart")]
            "dart" => Some(tree_sitter_dart::LANGUAGE.into()),

            #[cfg(feature = "lang-sql")]
            "sql" => Some(tree_sitter_sequel::LANGUAGE.into()),

            #[cfg(feature = "lang-kotlin")]
            "kotlin" => Some(tree_sitter_kotlin_sg::LANGUAGE.into()),

            // Phase 2 languages
            #[cfg(feature = "lang-csharp")]
            "csharp" => Some(tree_sitter_c_sharp::LANGUAGE.into()),

            #[cfg(feature = "lang-zig")]
            "zig" => Some(tree_sitter_zig::LANGUAGE.into()),

            #[cfg(feature = "lang-elixir")]
            "elixir" => Some(tree_sitter_elixir::LANGUAGE.into()),

            #[cfg(feature = "lang-r")]
            "r" => Some(tree_sitter_r::LANGUAGE.into()),

            #[cfg(feature = "lang-svelte")]
            "svelte" => Some(tree_sitter_svelte_ng::LANGUAGE.into()),

            #[cfg(feature = "lang-nix")]
            "nix" => Some(tree_sitter_nix::LANGUAGE.into()),

            #[cfg(feature = "lang-haskell")]
            "haskell" => Some(tree_sitter_haskell::LANGUAGE.into()),

            _ => None,
        }
    }

    pub fn get_highlight_query(name: &str) -> Option<&'static str> {
        match name {
            #[cfg(feature = "lang-rust")]
            "rust" => Some(tree_sitter_rust::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-javascript")]
            "javascript" | "jsx" => Some(JS_QUERY.as_str()),

            #[cfg(feature = "lang-typescript")]
            "typescript" | "tsx" => Some(TS_QUERY.as_str()),

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
            "css" => Some(CSS_HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-markdown")]
            "markdown" => Some(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),

            #[cfg(feature = "lang-bash")]
            "bash" => Some(tree_sitter_bash::HIGHLIGHT_QUERY),

            #[cfg(feature = "lang-ruby")]
            "ruby" => Some(tree_sitter_ruby::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-php")]
            "php" => Some(tree_sitter_php::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-yaml")]
            "yaml" => Some(tree_sitter_yaml::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-lua")]
            "lua" => Some(tree_sitter_lua::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-scala")]
            "scala" => Some(tree_sitter_scala::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-swift")]
            "swift" => Some(tree_sitter_swift::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-dart")]
            "dart" => Some(tree_sitter_dart::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-sql")]
            "sql" => Some(tree_sitter_sequel::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-kotlin")]
            "kotlin" => Some(tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY),

            // Phase 2 languages
            #[cfg(feature = "lang-csharp")]
            "csharp" => Some(CSHARP_HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-zig")]
            "zig" => Some(tree_sitter_zig::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-elixir")]
            "elixir" => Some(tree_sitter_elixir::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-r")]
            "r" => Some(tree_sitter_r::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-svelte")]
            "svelte" => Some(tree_sitter_svelte_ng::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-nix")]
            "nix" => Some(tree_sitter_nix::HIGHLIGHTS_QUERY),

            #[cfg(feature = "lang-haskell")]
            "haskell" => Some(tree_sitter_haskell::HIGHLIGHTS_QUERY),

            _ => None,
        }
    }

    pub fn get_injection_query(name: &str) -> Option<&'static str> {
        match name {
            #[cfg(feature = "lang-html")]
            "html" => Some(tree_sitter_html::INJECTIONS_QUERY),
            _ => None,
        }
    }
}

// Extended JS/TS queries: append string_fragment capture so template literal
// content between ${} expressions is highlighted as a string instead of plain text.
#[cfg(feature = "lang-javascript")]
static JS_QUERY: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}\n(string_fragment) @string\n",
        tree_sitter_javascript::HIGHLIGHT_QUERY
    )
});

#[cfg(feature = "lang-typescript")]
static TS_QUERY: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}\n(string_fragment) @string\n",
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    )
});

#[cfg(feature = "lang-css")]
const CSS_HIGHLIGHTS_QUERY: &str = r##"
; Comments
(comment) @comment

; Selectors
(tag_name) @tag
(nesting_selector) @tag
(universal_selector) @tag
(class_name) @property
(id_name) @property
(namespace_name) @property

; Pseudo-classes and pseudo-elements
(pseudo_element_selector (tag_name) @attribute)
(pseudo_class_selector (class_name) @attribute)

; Properties
(property_name) @property
(feature_name) @property

; Attribute selectors
(attribute_name) @attribute
(attribute_selector (plain_value) @string)

; Functions
(function_name) @function

; Values — color_value must be early so it captures #hex before "#" punctuation
(color_value) @string.special
(plain_value) @string
(string_value) @string
(integer_value) @number
(float_value) @number
(unit) @type
(important) @keyword
(grid_value) @string

; CSS custom properties (variables)
((property_name) @variable
 (#match? @variable "^--"))
((plain_value) @variable
 (#match? @variable "^--"))

; At-rules / keywords
"@media" @keyword
"@import" @keyword
"@charset" @keyword
"@namespace" @keyword
"@supports" @keyword
"@keyframes" @keyword
(at_keyword) @keyword
(to) @keyword
(from) @keyword

; Operators
"~" @operator
">" @operator
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"=" @operator
"^=" @operator
"|=" @operator
"~=" @operator
"$=" @operator
"*=" @operator
"and" @operator
"or" @operator
"not" @operator
"only" @operator

; Punctuation
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
[";" "," ":" "."] @punctuation.delimiter
"##;

#[cfg(feature = "lang-csharp")]
const CSHARP_HIGHLIGHTS_QUERY: &str = r#"
; Keywords
[
  "abstract" "as" "base" "break" "case" "catch" "checked" "class"
  "const" "continue" "default" "delegate" "do" "else" "enum" "event"
  "explicit" "extern" "finally" "fixed" "for" "foreach" "goto" "if"
  "implicit" "in" "interface" "internal" "is" "lock" "namespace" "new"
  "operator" "out" "override" "params" "private" "protected" "public"
  "readonly" "record" "ref" "return" "sealed" "sizeof" "stackalloc"
  "static" "struct" "switch" "this" "throw" "try" "typeof" "unchecked"
  "unsafe" "using" "virtual" "volatile" "while" "yield"
  "async" "await" "var" "get" "set" "init" "where" "when"
  "and" "or" "not" "with" "managed" "unmanaged" "notnull"
] @keyword

; Literals
(null_literal) @constant.builtin
(boolean_literal) @constant.builtin
(integer_literal) @number
(real_literal) @number
(character_literal) @string
(string_literal) @string
(verbatim_string_literal) @string
(interpolated_string_expression) @string
(raw_string_literal) @string

; Comments
(comment) @comment

; Types
(predefined_type) @type.builtin
(generic_name (identifier) @type)
(nullable_type (identifier) @type)
(array_type (identifier) @type)
(type (identifier) @type)

; Functions
(method_declaration name: (identifier) @function)
(local_function_statement name: (identifier) @function)
(invocation_expression function: (identifier) @function)
(invocation_expression function: (member_access_expression name: (identifier) @function.method))

; Constructors
(constructor_declaration name: (identifier) @constructor)
(object_creation_expression type: (identifier) @constructor)

; Properties and fields
(property_declaration name: (identifier) @property)
(field_declaration (variable_declaration (variable_declarator (identifier) @property)))

; Parameters
(parameter name: (identifier) @variable.parameter)

; Variables
(identifier) @variable

; Operators
[
  "+" "-" "*" "/" "%" "=" "+=" "-=" "*=" "/=" "%="
  "==" "!=" "<" ">" "<=" ">=" "&&" "||" "!"
  "&" "|" "^" "~" "<<" ">>" "??" "??="
  "=>" ".." "->" "++" "--"
] @operator

; Punctuation
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
[";" "," ":" "."] @punctuation.delimiter

; Attributes
(attribute name: (identifier) @attribute)
(attribute name: (qualified_name) @attribute)

; Namespace
(namespace_declaration name: (identifier) @type)
(using_directive (identifier) @type)
"#;
