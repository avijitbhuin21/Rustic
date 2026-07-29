//! Result checks that run over a *merged* state and refuse it when the merge
//! is textually clean but structurally broken.
//!
//! Two classes beyond `hidden`'s name-based dangling-reference check:
//! duplicate definitions (the same name defined twice in one scope) and
//! call-site arity mismatches (a signature changed on one side while the other
//! side added or kept a call using the old shape). Both are reported only when
//! the merge *introduces* them — anything already true of the base is the
//! base's problem, not the merge's, and reporting it would be noise.

use crate::parsers::parse;
use std::collections::{BTreeMap, BTreeSet};

/// Definition kinds excluded from the duplicate check because a repeated name
/// is legal for them (overload signatures).
fn overloadable(kind: &str) -> bool {
    matches!(kind, "function_signature" | "function_signature_item")
}

/// Definition kinds whose repeated appearance under one parent is an error.
fn unique_def_kinds(lang: &str) -> &'static [&'static str] {
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
        ],
        "python" => &["function_definition", "class_definition"],
        "javascript" => &[
            "function_declaration",
            "generator_function_declaration",
            "class_declaration",
            "method_definition",
        ],
        _ => &[
            "function_declaration",
            "generator_function_declaration",
            "class_declaration",
            "abstract_class_declaration",
            "interface_declaration",
            "enum_declaration",
            "type_alias_declaration",
            "method_definition",
        ],
    }
}

/// One declaration, as the duplicate check needs to see it. `scope` only has
/// to be consistent within a single text: two definitions are duplicates when
/// their scope, kind and name all match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub scope: String,
    pub kind: String,
    pub name: String,
}

/// Declarations of `text` that a repeated name would make illegal, or `None`
/// when no bundled grammar covers `lang`.
pub fn tree_definitions(lang: &str, text: &str) -> Option<Vec<Definition>> {
    let tree = parse(lang, text)?;
    let src = text.as_bytes();
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if unique_def_kinds(lang).contains(&kind) && !overloadable(kind) {
            if let (Some(name), Some(parent)) = (node.child_by_field_name("name"), node.parent()) {
                out.push(Definition {
                    scope: parent.id().to_string(),
                    kind: kind.to_string(),
                    name: String::from_utf8_lossy(&src[name.byte_range()]).into_owned(),
                });
            }
        }
        for i in 0..node.child_count() as u32 {
            stack.push(node.child(i).unwrap());
        }
    }
    Some(out)
}

/// Names defined more than once in one scope, as `kind:name`. The caller
/// decides which kinds are subject to the rule by what it puts in `defs`.
pub fn duplicates_in(defs: &[Definition]) -> BTreeSet<String> {
    let mut counts: BTreeMap<(&str, &str, &str), u32> = BTreeMap::new();
    for def in defs {
        *counts
            .entry((def.scope.as_str(), def.kind.as_str(), def.name.as_str()))
            .or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|((_, kind, name), _)| format!("{kind}:{name}"))
        .collect()
}

/// Names defined twice under the same parent node, as `kind:name`.
pub fn duplicate_definitions(lang: &str, text: &str) -> BTreeSet<String> {
    tree_definitions(lang, text)
        .map(|defs| duplicates_in(&defs))
        .unwrap_or_default()
}

/// Duplicate definitions the merged state has and the base did not, per file.
/// Duplicates are per-file by construction: the same name in two different
/// files is a different scope and perfectly legal.
pub fn new_duplicate_definitions(
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> BTreeSet<String> {
    let mut base = BTreeSet::new();
    for text in base_files {
        base.extend(duplicate_definitions(lang, text));
    }
    let mut merged = BTreeSet::new();
    for text in merged_files {
        merged.extend(duplicate_definitions(lang, text));
    }
    merged.difference(&base).cloned().collect()
}

/// The argument counts a callable accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arity {
    pub min: usize,
    pub max: Option<usize>,
}

impl Arity {
    /// True when a call with `count` positional arguments fits this signature.
    pub fn accepts(&self, count: usize) -> bool {
        count >= self.min && self.max.is_none_or(|max| count <= max)
    }

    /// A signature this check cannot read, which therefore accepts anything.
    pub fn any() -> Self {
        Arity { min: 0, max: None }
    }
}

/// One callable declaration: its name and the argument counts it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub arity: Arity,
}

/// One call site: the callee name and its positional argument count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub name: String,
    pub args: usize,
}

/// Parameter-list node kind for a language's callable declarations.
fn params_kind(lang: &str) -> &'static str {
    match lang {
        "rust" => "parameters",
        "python" => "parameters",
        _ => "formal_parameters",
    }
}

/// Count the parameters of a parameter-list node, or None when the shape is
/// not understood well enough to check calls against it.
fn count_params(lang: &str, params: tree_sitter::Node) -> Option<Arity> {
    let mut min = 0usize;
    let mut optional = 0usize;
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        match (lang, child.kind()) {
            (_, "comment" | "line_comment" | "block_comment") => {}
            ("rust", "parameter" | "self_parameter") => min += 1,
            ("rust", "variadic_parameter" | "...") => return None,
            ("rust", _) => return None,
            ("python", "identifier" | "typed_parameter" | "positional_separator") => min += 1,
            ("python", "default_parameter" | "typed_default_parameter") => optional += 1,
            ("python", _) => return None,
            (_, "required_parameter") => min += 1,
            (_, "optional_parameter") => optional += 1,
            (_, "identifier" | "object_pattern" | "array_pattern") => min += 1,
            (_, "assignment_pattern") => optional += 1,
            (_, _) => return None,
        }
    }
    Some(Arity {
        min,
        max: Some(min + optional),
    })
}

/// Callable declaration kinds whose plain-name calls we can check. Methods are
/// excluded: they are called through a receiver, which this check never reads.
fn callable_kinds(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &["function_item"],
        "python" => &["function_definition"],
        _ => &[
            "function_declaration",
            "generator_function_declaration",
            "function_signature",
        ],
    }
}

/// True when the node sits inside a type or class body, making it a method.
fn is_method(node: tree_sitter::Node) -> bool {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if matches!(
            n.kind(),
            "impl_item" | "trait_item" | "class_definition" | "class_body" | "class_declaration"
        ) {
            return true;
        }
        cur = n.parent();
    }
    false
}

/// Callable declarations in `text`, or `None` when no bundled grammar covers
/// `lang`.
pub fn tree_signatures(lang: &str, text: &str) -> Option<Vec<Signature>> {
    let tree = parse(lang, text)?;
    let src = text.as_bytes();
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if callable_kinds(lang).contains(&node.kind()) && !is_method(node) {
            let name = node.child_by_field_name("name");
            let params = node
                .child_by_field_name("parameters")
                .filter(|p| p.kind() == params_kind(lang));
            if let (Some(name), Some(params)) = (name, params) {
                out.push(Signature {
                    name: String::from_utf8_lossy(&src[name.byte_range()]).into_owned(),
                    // An unreadable parameter list must not manufacture a mismatch.
                    arity: count_params(lang, params).unwrap_or_else(Arity::any),
                });
            }
        }
        for i in 0..node.child_count() as u32 {
            stack.push(node.child(i).unwrap());
        }
    }
    Some(out)
}

/// Call-expression node kind for a language.
fn call_kind(lang: &str) -> &'static str {
    if lang == "python" {
        "call"
    } else {
        "call_expression"
    }
}

/// Count positional arguments of a call, or None when the call uses a shape
/// this check refuses to reason about (spreads, keyword arguments).
fn count_args(node: tree_sitter::Node) -> Option<usize> {
    let args = node.child_by_field_name("arguments")?;
    if !matches!(args.kind(), "arguments" | "argument_list") {
        return None;
    }
    let mut count = 0usize;
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        match child.kind() {
            "comment" | "line_comment" | "block_comment" => {}
            "spread_element" | "list_splat" | "dictionary_splat" | "keyword_argument" => {
                return None
            }
            _ => count += 1,
        }
    }
    Some(count)
}

/// Plain-name calls in `text` with their positional argument counts, or `None`
/// when no bundled grammar covers `lang`.
pub fn tree_call_sites(lang: &str, text: &str) -> Option<Vec<CallSite>> {
    let tree = parse(lang, text)?;
    let src = text.as_bytes();
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == call_kind(lang) {
            let callee = node.child_by_field_name("function");
            if let Some(callee) = callee.filter(|c| c.kind() == "identifier") {
                let name = String::from_utf8_lossy(&src[callee.byte_range()]).into_owned();
                if let Some(args) = count_args(node) {
                    out.push(CallSite { name, args });
                }
            }
        }
        for i in 0..node.child_count() as u32 {
            stack.push(node.child(i).unwrap());
        }
    }
    Some(out)
}

/// Call sites whose argument count no signature of the same name accepts, as
/// `name/argc`. A name with no signature at all is not reported: it may be
/// defined somewhere the caller did not supply.
pub fn arity_mismatches_in(sigs: &[Signature], calls: &[CallSite]) -> BTreeSet<String> {
    let mut by_name: BTreeMap<&str, Vec<Arity>> = BTreeMap::new();
    for sig in sigs {
        by_name
            .entry(sig.name.as_str())
            .or_default()
            .push(sig.arity);
    }
    let mut out = BTreeSet::new();
    for call in calls {
        let Some(arities) = by_name.get(call.name.as_str()) else {
            continue;
        };
        if !arities.iter().any(|a| a.accepts(call.args)) {
            out.insert(format!("{}/{}", call.name, call.args));
        }
    }
    out
}

/// Call sites in `texts` whose argument count no declaration in `texts`
/// accepts, as `name/argc`. Cross-file: definitions and calls are pooled.
pub fn arity_mismatches(lang: &str, texts: &[&str]) -> BTreeSet<String> {
    let mut sigs = Vec::new();
    let mut calls = Vec::new();
    for text in texts {
        sigs.extend(tree_signatures(lang, text).unwrap_or_default());
        calls.extend(tree_call_sites(lang, text).unwrap_or_default());
    }
    arity_mismatches_in(&sigs, &calls)
}

/// Arity mismatches the merged state has and the base did not.
pub fn new_arity_mismatches(
    lang: &str,
    base_files: &[&str],
    merged_files: &[&str],
) -> BTreeSet<String> {
    let base = arity_mismatches(lang, base_files);
    arity_mismatches(lang, merged_files)
        .difference(&base)
        .cloned()
        .collect()
}
