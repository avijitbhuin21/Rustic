//! Symbol entry + kind enum used by the workspace symbol index.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What kind of declaration the symbol points at. Coarse-grained across
/// languages so the agent's tools can present a consistent vocabulary;
/// callers that need finer detail can fall back to tree-sitter node kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    TypeAlias,
    Module,
    Variable,
    Constant,
    Macro,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Module => "module",
            SymbolKind::Variable => "variable",
            SymbolKind::Constant => "constant",
            SymbolKind::Macro => "macro",
        }
    }

    /// Parse a user-supplied filter string into a kind. Case-insensitive;
    /// accepts both the canonical name and a few common synonyms ("fn" → function,
    /// "ty" → type, etc.).
    pub fn from_str(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "function" | "fn" | "func" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" => SymbolKind::Class,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "interface" => SymbolKind::Interface,
            "type" | "typealias" | "ty" => SymbolKind::TypeAlias,
            "module" | "mod" | "namespace" => SymbolKind::Module,
            "variable" | "var" | "let" => SymbolKind::Variable,
            "constant" | "const" => SymbolKind::Constant,
            "macro" => SymbolKind::Macro,
            _ => return None,
        })
    }
}

/// One declaration found in the project. Stored in the symbol index keyed by
/// `name`, and also reverse-indexed by file so single-file refreshes can
/// remove the file's old entries before inserting new ones.
///
/// Positions are 1-indexed (line and column) so they match the editor's
/// gutter and what we surface to the model in other tools (`read_file`,
/// `grep_search`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub name: String,
    pub file: PathBuf,
    pub line: u32,
    pub col: u32,
    pub kind: SymbolKind,
    /// Enclosing scope (e.g. `impl Foo` for a Rust method, or the parent
    /// class name for a Python method). `None` for top-level items.
    pub scope: Option<String>,
}

impl SymbolEntry {
    /// Compact one-line representation used by the find_symbol /
    /// goto_definition tool output. `path` is the project-relative form.
    pub fn render_line(&self, project_relative: &str) -> String {
        match &self.scope {
            Some(scope) => format!(
                "{}:{}:{} ({} in {}) — {}",
                project_relative,
                self.line,
                self.col,
                self.kind.as_str(),
                scope,
                self.name
            ),
            None => format!(
                "{}:{}:{} ({}) — {}",
                project_relative,
                self.line,
                self.col,
                self.kind.as_str(),
                self.name
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // L9.1 — SymbolKind::from_str synonyms.
    #[test]
    fn from_str_accepts_canonical_names() {
        assert_eq!(SymbolKind::from_str("function"), Some(SymbolKind::Function));
        assert_eq!(SymbolKind::from_str("method"), Some(SymbolKind::Method));
        assert_eq!(SymbolKind::from_str("class"), Some(SymbolKind::Class));
        assert_eq!(SymbolKind::from_str("struct"), Some(SymbolKind::Struct));
        assert_eq!(SymbolKind::from_str("enum"), Some(SymbolKind::Enum));
        assert_eq!(SymbolKind::from_str("trait"), Some(SymbolKind::Trait));
        assert_eq!(
            SymbolKind::from_str("interface"),
            Some(SymbolKind::Interface)
        );
        assert_eq!(SymbolKind::from_str("type"), Some(SymbolKind::TypeAlias));
        assert_eq!(SymbolKind::from_str("module"), Some(SymbolKind::Module));
        assert_eq!(SymbolKind::from_str("variable"), Some(SymbolKind::Variable));
        assert_eq!(SymbolKind::from_str("constant"), Some(SymbolKind::Constant));
        assert_eq!(SymbolKind::from_str("macro"), Some(SymbolKind::Macro));
    }

    #[test]
    fn from_str_accepts_short_synonyms() {
        // Function synonyms.
        assert_eq!(SymbolKind::from_str("fn"), Some(SymbolKind::Function));
        assert_eq!(SymbolKind::from_str("func"), Some(SymbolKind::Function));
        // Type-alias synonyms.
        assert_eq!(SymbolKind::from_str("ty"), Some(SymbolKind::TypeAlias));
        assert_eq!(
            SymbolKind::from_str("typealias"),
            Some(SymbolKind::TypeAlias)
        );
        // Module synonyms.
        assert_eq!(SymbolKind::from_str("mod"), Some(SymbolKind::Module));
        assert_eq!(SymbolKind::from_str("namespace"), Some(SymbolKind::Module));
        // Variable synonyms.
        assert_eq!(SymbolKind::from_str("var"), Some(SymbolKind::Variable));
        assert_eq!(SymbolKind::from_str("let"), Some(SymbolKind::Variable));
        // Constant synonym.
        assert_eq!(SymbolKind::from_str("const"), Some(SymbolKind::Constant));
    }

    #[test]
    fn from_str_is_case_insensitive_and_trims() {
        assert_eq!(SymbolKind::from_str("Function"), Some(SymbolKind::Function));
        assert_eq!(SymbolKind::from_str("FUNCTION"), Some(SymbolKind::Function));
        assert_eq!(SymbolKind::from_str("  fn  "), Some(SymbolKind::Function));
        assert_eq!(SymbolKind::from_str("MoD"), Some(SymbolKind::Module));
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert_eq!(SymbolKind::from_str(""), None);
        assert_eq!(SymbolKind::from_str("nonsense"), None);
        assert_eq!(SymbolKind::from_str("classs"), None);
        assert_eq!(SymbolKind::from_str("functions"), None);
    }

    // L9.2 — SymbolEntry::render_line formatting.
    fn entry_with_scope(scope: Option<&str>) -> SymbolEntry {
        SymbolEntry {
            name: "do_thing".into(),
            file: PathBuf::from("/abs/whatever/src/x.rs"),
            line: 42,
            col: 7,
            kind: SymbolKind::Method,
            scope: scope.map(|s| s.to_string()),
        }
    }

    #[test]
    fn render_line_with_scope_includes_in_scope_clause() {
        let e = entry_with_scope(Some("impl Foo"));
        assert_eq!(
            e.render_line("src/x.rs"),
            "src/x.rs:42:7 (method in impl Foo) — do_thing"
        );
    }

    #[test]
    fn render_line_without_scope_omits_in_clause() {
        let e = entry_with_scope(None);
        assert_eq!(
            e.render_line("src/x.rs"),
            "src/x.rs:42:7 (method) — do_thing"
        );
    }

    #[test]
    fn render_line_passes_through_project_relative_unchanged() {
        // `render_line` doesn't recompute the relative path — caller owns it.
        let e = entry_with_scope(None);
        assert!(e
            .render_line("anything/at/all.txt")
            .starts_with("anything/at/all.txt:42:7"));
    }

    #[test]
    fn render_line_renders_each_kind_canonically() {
        let mut e = entry_with_scope(None);
        for (k, label) in [
            (SymbolKind::Function, "function"),
            (SymbolKind::Method, "method"),
            (SymbolKind::Class, "class"),
            (SymbolKind::Struct, "struct"),
            (SymbolKind::Enum, "enum"),
            (SymbolKind::Trait, "trait"),
            (SymbolKind::Interface, "interface"),
            (SymbolKind::TypeAlias, "type"),
            (SymbolKind::Module, "module"),
            (SymbolKind::Variable, "variable"),
            (SymbolKind::Constant, "constant"),
            (SymbolKind::Macro, "macro"),
        ] {
            e.kind = k;
            let line = e.render_line("f.rs");
            assert!(
                line.contains(&format!("({})", label)),
                "expected `({})` in `{}`",
                label,
                line
            );
        }
    }
}
