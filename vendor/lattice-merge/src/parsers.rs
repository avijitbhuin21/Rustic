//! Shared tree-sitter parsing helpers for the merge stack (port of
//! `phase0/structural/parsers.py`).

use std::collections::BTreeSet;

/// Language id for a path, if the merge stack supports it.
pub fn lang_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit('.').next()?;
    match ext {
        "rs" => Some("rust"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "py" | "pyi" => Some("python"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        _ => None,
    }
}

fn language(lang: &str) -> Option<tree_sitter::Language> {
    match lang {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        _ => None,
    }
}

/// Whether a bundled grammar covers `lang`. Callers that must not silently
/// skip a structural check should gate on this.
pub fn supported(lang: &str) -> bool {
    language(lang).is_some()
}

/// Parse source text with the named grammar.
pub fn parse(lang: &str, text: &str) -> Option<tree_sitter::Tree> {
    PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language(lang)?).ok()?;
    parser.parse(text, None)
}

static PARSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total grammar invocations since process start — the memoization guard in
/// the test suite asserts against this rather than against wall-clock time.
pub fn parse_count() -> u64 {
    PARSES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Parse and reject anything with ERROR or MISSING nodes.
pub fn parse_clean(lang: &str, text: &str) -> Option<tree_sitter::Tree> {
    parse(lang, text).filter(|t| !t.root_node().has_error())
}

/// Import-declaration kinds, whose additions union instead of conflicting.
pub fn import_kinds(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &["use_declaration", "extern_crate_declaration"],
        "python" => &[
            "import_statement",
            "import_from_statement",
            "future_import_statement",
        ],
        _ => &["import_statement", "import_alias"],
    }
}

/// True when the text parses without ERROR or MISSING nodes.
pub fn parses_clean(lang: &str, text: &str) -> bool {
    parse(lang, text).is_some_and(|t| !t.root_node().has_error())
}

/// Node kinds whose direct children commute (order-insensitive containers).
pub fn commutative_parents(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &[
            "source_file",
            "declaration_list",
            "field_declaration_list",
            "enum_variant_list",
        ],
        // Python: only module-level defs commute; suites are order-sensitive.
        "python" => &["module"],
        _ => &[
            "program",
            "class_body",
            "enum_body",
            "object_type",
            "interface_body",
        ],
    }
}

/// Trivia node kinds attached to the following declaration.
pub const TRIVIA_TYPES: &[&str] = &[
    "line_comment",
    "block_comment",
    "comment",
    "attribute_item",
    "inner_attribute_item",
];

/// Declaration kinds that carry a `name` field (for reference tracking).
pub fn def_name_types(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &[
            "function_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "const_item",
            "static_item",
            "type_item",
            "mod_item",
            "union_item",
            "macro_definition",
            "function_signature_item",
        ],
        "python" => &["function_definition", "class_definition"],
        "javascript" => &[
            "function_declaration",
            "generator_function_declaration",
            "class_declaration",
            "method_definition",
            "variable_declarator",
        ],
        _ => &[
            "function_declaration",
            "class_declaration",
            "abstract_class_declaration",
            "interface_declaration",
            "enum_declaration",
            "type_alias_declaration",
            "method_definition",
            "variable_declarator",
            "function_signature",
            "public_field_definition",
        ],
    }
}

const IDENT_TYPES: &[&str] = &[
    "identifier",
    "type_identifier",
    "field_identifier",
    "property_identifier",
    "shorthand_property_identifier",
];

/// Language-agnostic token stream for cheap formatting-only hunk checks.
pub fn rough_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
        } else {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            if !ch.is_whitespace() {
                out.push(ch.to_string());
            }
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Names of declarations anywhere in the file (top-level and nested).
/// Rewrite every identifier token equal to `old` into `new`. Returns None
/// when the file does not parse or contains no such identifier.
pub fn rewrite_identifiers(lang: &str, text: &str, old: &str, new: &str) -> Option<String> {
    let tree = parse(lang, text)?;
    let src = text.as_bytes();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            if node.kind().contains("identifier") && &src[node.byte_range()] == old.as_bytes() {
                ranges.push(node.byte_range());
            }
            continue;
        }
        for i in 0..node.child_count() as u32 {
            stack.push(node.child(i).unwrap());
        }
    }
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_by_key(|r| r.start);
    let mut out = text.to_string();
    for range in ranges.into_iter().rev() {
        out.replace_range(range, new);
    }
    Some(out)
}

pub fn defined_names(lang: &str, text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(tree) = parse(lang, text) else {
        return names;
    };
    let src = text.as_bytes();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if def_name_types(lang).contains(&node.kind()) {
            if let Some(name) = node.child_by_field_name("name") {
                names.insert(String::from_utf8_lossy(&src[name.byte_range()]).into_owned());
            }
        }
        for i in 0..node.child_count() as u32 {
            stack.push(node.child(i).unwrap());
        }
    }
    names
}

/// Identifier leaves that are not themselves declaration names.
pub fn referenced_names(lang: &str, text: &str) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    let Some(tree) = parse(lang, text) else {
        return refs;
    };
    let src = text.as_bytes();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 && IDENT_TYPES.contains(&node.kind()) {
            let is_def_name = node.parent().is_some_and(|p| {
                def_name_types(lang).contains(&p.kind())
                    && p.child_by_field_name("name").map(|n| n.id()) == Some(node.id())
            });
            if !is_def_name {
                refs.insert(String::from_utf8_lossy(&src[node.byte_range()]).into_owned());
            }
        }
        for i in 0..node.child_count() as u32 {
            stack.push(node.child(i).unwrap());
        }
    }
    refs
}
