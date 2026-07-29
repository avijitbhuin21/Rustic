//! A `lattice_merge::SymbolExtractor` backed by Rustic's own tree-sitter
//! grammars and tags queries.
//!
//! Why not use lattice's built-in extractor: it parses five languages
//! (rust, typescript, tsx, javascript, python). Rustic ships grammars and tags
//! queries for nineteen. Since hidden-conflict detection is purely name-based,
//! the extra grammars extend dangling-reference detection to Go, Java, C/C++,
//! Ruby, PHP, C#, Kotlin, Swift, Scala and the declarative languages at no
//! extra cost.
//!
//! `defined` reuses the exact tags queries the symbol index is built from, so
//! "what defines a name" means the same thing here as in `find_symbol`.
//! `referenced` has no query behind it — Rustic's tags queries capture
//! declarations only — so it walks the tree and collects identifier-shaped
//! leaves that aren't declaration sites. That over-collects (a field access and
//! a local both look like identifiers), which is the safe direction: an extra
//! reference can only make a merge look *less* clean, never more.

use std::collections::BTreeSet;

use lattice_merge::SymbolExtractor;
use tree_sitter::{Node, Query, QueryCursor};

use crate::index::queries::{kind_from_capture, query_source};

/// Cap on text handed to the parser. Above this the extractor reports nothing,
/// which makes the merge look unverified rather than clean.
const MAX_EXTRACT_BYTES: usize = 512 * 1024;

/// Symbol extraction over Rustic's grammars + tags queries.
pub(crate) struct IndexExtractor;

impl SymbolExtractor for IndexExtractor {
    fn defined(&self, lang: &str, text: &str) -> BTreeSet<String> {
        defined_names(lang, text).unwrap_or_default()
    }

    fn referenced(&self, lang: &str, text: &str) -> BTreeSet<String> {
        referenced_names(lang, text).unwrap_or_default()
    }
}

/// Node kinds whose text is an identifier worth treating as a reference.
fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "simple_identifier"
            | "constant"
            | "name"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
    )
}

/// Parse `text` as `lang`, returning the tree and the byte slice it borrows.
fn parse(lang: &str, text: &str) -> Option<(tree_sitter::Tree, tree_sitter::Language)> {
    if text.len() > MAX_EXTRACT_BYTES {
        return None;
    }
    let language = rustic_treesitter::LanguageRegistry::get_language(lang)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;
    Some((tree, language))
}

/// Names declared by `text`, via the same tags query the symbol index uses.
fn defined_names(lang: &str, text: &str) -> Option<BTreeSet<String>> {
    let query_src = query_source(lang)?;
    let (tree, language) = parse(lang, text)?;
    let query = Query::new(&language, query_src).ok()?;
    let capture_names: Vec<String> = query
        .capture_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    let bytes = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    use streaming_iterator::StreamingIterator as _;
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let Some(cname) = capture_names.get(cap.index as usize) else {
                continue;
            };
            if kind_from_capture(cname).is_none() {
                continue;
            }
            if let Ok(name) = cap.node.utf8_text(bytes) {
                let name = name.trim();
                if !name.is_empty() {
                    out.insert(name.to_string());
                }
            }
        }
    }
    Some(out)
}

/// Identifier-shaped leaves that are not declaration sites.
fn referenced_names(lang: &str, text: &str) -> Option<BTreeSet<String>> {
    let (tree, _) = parse(lang, text)?;
    let declared = defined_names(lang, text).unwrap_or_default();
    let bytes = text.as_bytes();

    let mut out = BTreeSet::new();
    let mut stack: Vec<Node> = vec![tree.root_node()];
    let mut cursor = tree.walk();
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            if is_identifier_kind(node.kind()) {
                if let Ok(name) = node.utf8_text(bytes) {
                    let name = name.trim();
                    if !name.is_empty() && !declared.contains(name) {
                        out.insert(name.to_string());
                    }
                }
            }
            continue;
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_definitions_are_found() {
        let src = "struct Widget;\n\nfn build(w: Widget) -> Widget {\n    w\n}\n";
        let defined = IndexExtractor.defined("rust", src);
        assert!(defined.contains("Widget"), "got {:?}", defined);
        assert!(defined.contains("build"), "got {:?}", defined);
    }

    #[test]
    fn rust_references_exclude_own_declarations() {
        let src = "fn caller() {\n    helper(1);\n}\n";
        let refs = IndexExtractor.referenced("rust", src);
        assert!(refs.contains("helper"), "got {:?}", refs);
        assert!(
            !refs.contains("caller"),
            "a declaration is not a reference: {:?}",
            refs
        );
    }

    /// The point of the whole extractor: a language lattice cannot parse.
    #[test]
    fn go_is_covered_even_though_lattice_cannot_parse_it() {
        assert!(
            lattice_merge::lang_for_path("main.go").is_none(),
            "if lattice gains a go parser this test's premise changed"
        );
        let src =
            "package main\n\nfunc helper() int {\n\treturn 1\n}\n\nfunc main() {\n\thelper()\n}\n";
        let defined = IndexExtractor.defined("go", src);
        assert!(defined.contains("helper"), "got {:?}", defined);
        let refs = IndexExtractor.referenced("go", src);
        assert!(
            !refs.contains("helper"),
            "helper is declared here: {:?}",
            refs
        );
    }

    #[test]
    fn python_definitions_and_references() {
        let src = "def helper(x):\n    return x\n\ndef main():\n    return other(helper(1))\n";
        let defined = IndexExtractor.defined("python", src);
        assert!(
            defined.contains("helper") && defined.contains("main"),
            "got {:?}",
            defined
        );
        let refs = IndexExtractor.referenced("python", src);
        assert!(refs.contains("other"), "got {:?}", refs);
    }

    #[test]
    fn unknown_language_reports_nothing() {
        assert!(IndexExtractor.defined("brainfuck", "+++").is_empty());
        assert!(IndexExtractor.referenced("brainfuck", "+++").is_empty());
    }

    #[test]
    fn oversized_text_reports_nothing() {
        let big = "fn a() {}\n".repeat(MAX_EXTRACT_BYTES);
        assert!(IndexExtractor.defined("rust", &big).is_empty());
    }
}
