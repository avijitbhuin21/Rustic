//! Stable symbol identities (§3.2): named declarations carry ids that
//! survive renames and moves; matching runs body-hash rename/move detection.

use crate::hash::Hash;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// A declaration extracted from one file, before identity assignment.
#[derive(Clone, Debug, PartialEq)]
pub struct RawDecl {
    pub name: String,
    pub kind: String,
    pub body_hash: Hash,
}

/// A symbol id derived from the declaration itself rather than drawn at random.
///
/// Reconciling a fresh table over the same texts must produce the same ids:
/// anything keyed on them (the algebra's `State::decls`, for one) would
/// otherwise iterate in a different order every run, making merges
/// irreproducible. `nonce` disambiguates the rare case of two declarations
/// sharing a path, kind, name *and* body hash.
fn fresh_symbol_id(path: &str, decl: &RawDecl, taken: &HashSet<String>) -> String {
    for nonce in 0u32.. {
        let mut buf = Vec::new();
        buf.extend_from_slice(path.as_bytes());
        buf.push(0);
        buf.extend_from_slice(decl.kind.as_bytes());
        buf.push(0);
        buf.extend_from_slice(decl.name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(decl.body_hash.0.as_bytes());
        if nonce > 0 {
            buf.push(0);
            buf.extend_from_slice(&nonce.to_le_bytes());
        }
        let id = Hash::of_bytes(&buf).0;
        if !taken.contains(&id) {
            return id;
        }
    }
    unreachable!("u32 nonce space exhausted for one declaration")
}

/// A tracked symbol: identity is the `id`, everything else is metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub body_hash: Hash,
    pub live: bool,
}

/// The persistent symbol identity table.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SymbolTable {
    pub symbols: Vec<Symbol>,
}

/// What happened to a symbol between two snapshots.
#[derive(Clone, Debug, PartialEq)]
pub enum SymbolEvent {
    Added {
        id: String,
        path: String,
        name: String,
        kind: String,
    },
    Deleted {
        id: String,
        path: String,
        name: String,
    },
    BodyModified {
        id: String,
        path: String,
        name: String,
    },
    Renamed {
        id: String,
        path: String,
        old_name: String,
        new_name: String,
    },
    Moved {
        id: String,
        name: String,
        old_path: String,
        new_path: String,
    },
}

impl SymbolTable {
    /// Find a live symbol by its stable id.
    pub fn get(&self, id: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.id == id)
    }

    /// Find live symbols by current name.
    pub fn find_by_name(&self, name: &str) -> Vec<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.live && s.name == name)
            .collect()
    }

    /// Reconcile the table against a fresh extraction (path -> decls),
    /// preserving identities across renames (same body, new name) and moves
    /// (same name+body, new file). Returns the symbol events observed.
    pub fn reconcile(&mut self, extracted: &BTreeMap<String, Vec<RawDecl>>) -> Vec<SymbolEvent> {
        let mut events = Vec::new();
        let mut unmatched_new: Vec<(String, RawDecl)> = Vec::new();
        let mut matched_ids: HashSet<String> = HashSet::new();

        for (path, decls) in extracted {
            for decl in decls {
                let found = self.symbols.iter_mut().find(|s| {
                    s.live
                        && s.path == *path
                        && s.name == decl.name
                        && s.kind == decl.kind
                        && !matched_ids.contains(&s.id)
                });
                if let Some(sym) = found {
                    if sym.body_hash != decl.body_hash {
                        events.push(SymbolEvent::BodyModified {
                            id: sym.id.clone(),
                            path: path.clone(),
                            name: sym.name.clone(),
                        });
                        sym.body_hash = decl.body_hash.clone();
                    }
                    matched_ids.insert(sym.id.clone());
                } else {
                    unmatched_new.push((path.clone(), decl.clone()));
                }
            }
        }

        let mut still_new = Vec::new();
        for (path, decl) in unmatched_new {
            let rename = self.symbols.iter_mut().find(|s| {
                s.live
                    && s.path == path
                    && s.kind == decl.kind
                    && s.body_hash == decl.body_hash
                    && !matched_ids.contains(&s.id)
                    && !extracted
                        .get(&s.path)
                        .is_some_and(|ds| ds.iter().any(|d| d.name == s.name && d.kind == s.kind))
            });
            if let Some(sym) = rename {
                events.push(SymbolEvent::Renamed {
                    id: sym.id.clone(),
                    path: path.clone(),
                    old_name: sym.name.clone(),
                    new_name: decl.name.clone(),
                });
                sym.name = decl.name.clone();
                matched_ids.insert(sym.id.clone());
                continue;
            }
            // `extracted` only covers the files this reconcile looked at, so a
            // symbol whose file was never re-extracted is still where it was
            // and must not be claimed as the origin of a move.
            let moved = self.symbols.iter_mut().find(|s| {
                s.live
                    && s.path != path
                    && s.kind == decl.kind
                    && s.name == decl.name
                    && s.body_hash == decl.body_hash
                    && !matched_ids.contains(&s.id)
                    && extracted.contains_key(&s.path)
                    && !extracted
                        .get(&s.path)
                        .is_some_and(|ds| ds.iter().any(|d| d.name == s.name && d.kind == s.kind))
            });
            if let Some(sym) = moved {
                events.push(SymbolEvent::Moved {
                    id: sym.id.clone(),
                    name: sym.name.clone(),
                    old_path: sym.path.clone(),
                    new_path: path.clone(),
                });
                sym.path = path.clone();
                matched_ids.insert(sym.id.clone());
                continue;
            }
            still_new.push((path, decl));
        }

        let mut taken: HashSet<String> = self.symbols.iter().map(|s| s.id.clone()).collect();
        for (path, decl) in still_new {
            let id = fresh_symbol_id(&path, &decl, &taken);
            taken.insert(id.clone());
            events.push(SymbolEvent::Added {
                id: id.clone(),
                path: path.clone(),
                name: decl.name.clone(),
                kind: decl.kind.clone(),
            });
            self.symbols.push(Symbol {
                id,
                name: decl.name,
                path,
                kind: decl.kind,
                body_hash: decl.body_hash,
                live: true,
            });
            matched_ids.insert(self.symbols.last().unwrap().id.clone());
        }

        for sym in &mut self.symbols {
            if sym.live && extracted.contains_key(&sym.path) && !matched_ids.contains(&sym.id) {
                sym.live = false;
                events.push(SymbolEvent::Deleted {
                    id: sym.id.clone(),
                    path: sym.path.clone(),
                    name: sym.name.clone(),
                });
            }
        }
        events
    }
}

thread_local! {
    /// Building a `Parser` and re-setting its language costs more than the
    /// parse itself on small files, and `extract_decls` runs once per changed
    /// file (PERF-3). One parser per language per thread is reused instead.
    static PARSERS: std::cell::RefCell<BTreeMap<&'static str, tree_sitter::Parser>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// Parse `text` with a reused parser for `grammar`, returning None when the
/// grammar is unsupported or the parse fails.
fn parse_with(grammar: &'static str, text: &str) -> Option<tree_sitter::Tree> {
    PARSERS.with(|cell| {
        let mut parsers = cell.borrow_mut();
        if !parsers.contains_key(grammar) {
            let language = match grammar {
                "rust" => tree_sitter::Language::from(tree_sitter_rust::LANGUAGE),
                "typescript" => {
                    tree_sitter::Language::from(tree_sitter_typescript::LANGUAGE_TYPESCRIPT)
                }
                _ => return None,
            };
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&language).ok()?;
            parsers.insert(grammar, parser);
        }
        parsers.get_mut(grammar)?.parse(text, None)
    })
}

/// Grammar name for a file name, mirroring the CST grammar registry's
/// extension table. Kept separate from `parsers::lang_for_path`, which maps
/// `.tsx` to its own `tsx` language id; declaration extraction parses `.tsx`
/// with the TypeScript grammar, and narrowing that would silently drop the
/// symbol identities of every `.tsx` file.
fn grammar_name(file_name: &str) -> Option<&'static str> {
    match file_name.rsplit('.').next()? {
        "rs" => Some("rust"),
        "ts" | "tsx" | "mts" | "cts" => Some("typescript"),
        "py" | "pyi" => Some("python"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        _ => None,
    }
}

/// Extract named top-level declarations from source text with tree-sitter.
pub fn extract_decls(file_name: &str, text: &str) -> Vec<RawDecl> {
    let Some(grammar) = grammar_name(file_name) else {
        return Vec::new();
    };
    let Some(tree) = parse_with(grammar, text) else {
        return Vec::new();
    };
    crate::metrics::bump_decl_parse();
    let src = text.as_bytes();
    let mut out = Vec::new();
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().children(&mut cursor) {
        let kind = node.kind();
        let interesting = matches!(
            kind,
            "function_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "mod_item"
                | "const_item"
                | "static_item"
                | "type_item"
                | "function_declaration"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
        );
        if !interesting {
            continue;
        }
        let Some(name_node) = node.child_by_field_name("name") else {
            continue;
        };
        let name = String::from_utf8_lossy(&src[name_node.byte_range()]).to_string();
        let body = node
            .child_by_field_name("body")
            .map(|b| b.byte_range())
            .unwrap_or_else(|| node.byte_range());
        out.push(RawDecl {
            name,
            kind: kind.to_string(),
            body_hash: Hash::of_bytes(&src[body]),
        });
    }
    out
}
