//! Three-way structural file merge (port of
//! `phase0/structural/structural_merge.py`): line-merge fast path, hunk
//! strategies, then declaration-level merge — CST machinery is only paid for
//! on residual conflicts.
//!
//! The declaration merge is recursive (Tier R): when a declaration is dirty on
//! both sides it is re-merged over the declarations inside its own body, so two
//! edits in different methods of one `impl` block compose instead of
//! conflicting. Recursion stops at `MAX_RECURSION` and at anything it cannot
//! key by name.
//!
//! Every revision is parsed exactly once per merge and the trees are threaded
//! through the pipeline; no strategy re-parses base, left or right.

use crate::linemerge::{line_merge, parse_markers, render, Hunk, Segment};
use crate::parsers::{
    commutative_parents, import_kinds, parse_clean, parses_clean, rough_tokens, TRIVIA_TYPES,
};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

/// How deep the declaration merge recurses into nested bodies.
const MAX_RECURSION: usize = 3;

/// Result of a structural file merge.
#[derive(Clone, Debug)]
pub struct MergeOutcome {
    pub status: MergeStatus,
    pub text: String,
    pub strategies: BTreeMap<String, u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeStatus {
    Clean,
    Conflict,
    ParseFallback,
}

fn bump(stats: &mut BTreeMap<String, u32>, key: &str) {
    *stats.entry(key.to_string()).or_insert(0) += 1;
}

fn base_prefix_lines(doc: &[Segment], upto: usize) -> Vec<String> {
    let mut out = Vec::new();
    for seg in &doc[..upto] {
        match seg {
            Segment::Context(lines) => out.extend(lines.clone()),
            Segment::Conflict(h) => out.extend(h.base.clone()),
        }
    }
    out
}

fn enclosing_commutative(base_tree: &tree_sitter::Tree, offset: usize, lang: &str) -> bool {
    let root = base_tree.root_node();
    let mut node = root.descendant_for_byte_range(offset, offset);
    while let Some(n) = node {
        if commutative_parents(lang).contains(&n.kind()) {
            return true;
        }
        node = n.parent();
    }
    false
}

fn resolve_hunk(
    hunk: &Hunk,
    doc: &[Segment],
    idx: usize,
    base_tree: &tree_sitter::Tree,
    lang: &str,
    stats: &mut BTreeMap<String, u32>,
) -> Option<Vec<String>> {
    if hunk.ours == hunk.theirs {
        bump(stats, "identical");
        return Some(hunk.ours.clone());
    }
    let ours_t = rough_tokens(&hunk.ours.join("\n"));
    let base_t = rough_tokens(&hunk.base.join("\n"));
    let theirs_t = rough_tokens(&hunk.theirs.join("\n"));
    if ours_t == base_t {
        bump(stats, "format_only");
        return Some(hunk.theirs.clone());
    }
    if theirs_t == base_t {
        bump(stats, "format_only");
        return Some(hunk.ours.clone());
    }
    if ours_t == theirs_t {
        bump(stats, "format_only");
        return Some(hunk.ours.clone());
    }
    if hunk.base.is_empty() {
        let prefix = base_prefix_lines(doc, idx).join("\n");
        let offset = prefix.len() + usize::from(!prefix.is_empty());
        if enclosing_commutative(base_tree, offset, lang) {
            bump(stats, "commutative_insert");
            return Some(union_insertions(&hunk.ours, &hunk.theirs, stats));
        }
    }
    None
}

/// True when every non-blank line of `needle` occurs as one contiguous run inside `haystack`.
fn contains_run(haystack: &[String], needle: &[String]) -> bool {
    let n: Vec<&str> = needle
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    let h: Vec<&str> = haystack
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    if n.is_empty() || n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w == n.as_slice())
}

/// Union two independent insertions, dropping a side the other already spells out verbatim.
///
/// A commutative container makes the two insertions order-independent, but it does not make
/// them disjoint: both sides frequently add the *same* declaration, and concatenating then
/// emits it twice. Containment is checked rather than assumed, so nothing is ever invented.
fn union_insertions(
    ours: &[String],
    theirs: &[String],
    stats: &mut BTreeMap<String, u32>,
) -> Vec<String> {
    if contains_run(ours, theirs) {
        bump(stats, "insert_dedup");
        return ours.to_vec();
    }
    if contains_run(theirs, ours) {
        bump(stats, "insert_dedup");
        return theirs.to_vec();
    }
    let mut both = ours.to_vec();
    both.extend_from_slice(theirs);
    both
}

fn node_key(node: tree_sitter::Node, src: &[u8]) -> (String, bool) {
    if node.kind() == "export_statement" {
        let decl = node.child_by_field_name("declaration").or_else(|| {
            (0..node.named_child_count() as u32)
                .map(|i| node.named_child(i).unwrap())
                .find(|c| {
                    let k = c.kind();
                    (k.ends_with("declaration")
                        || k.ends_with("_statement")
                        || k.ends_with("function")
                        || k.ends_with("class"))
                        && k != "string"
                })
        });
        if let Some(decl) = decl {
            let (inner, anon) = node_key(decl, src);
            return (format!("export+{inner}"), anon);
        }
    }
    if let Some(name) = node.child_by_field_name("name") {
        let name_text = String::from_utf8_lossy(&src[name.byte_range()]);
        return (format!("{}:{}", node.kind(), name_text), false);
    }
    if node.kind() == "impl_item" {
        let header_end = node
            .child_by_field_name("body")
            .map(|b| b.start_byte())
            .unwrap_or_else(|| node.end_byte());
        let header = String::from_utf8_lossy(&src[node.start_byte()..header_end]);
        let squashed = header.split_whitespace().collect::<Vec<_>>().join(" ");
        return (format!("{}:{}", node.kind(), squashed), false);
    }
    let toks = rough_tokens(&String::from_utf8_lossy(&src[node.byte_range()])).join(" ");
    (format!("{}:{}", node.kind(), toks), true)
}

/// One child of a declaration container, carrying the trivia that precedes it.
#[derive(Clone, Debug)]
struct Chunk {
    key: String,
    text: String,
    /// Keyed by token content because the node has no name — order-sensitive,
    /// so it may not be reordered or unioned.
    anon: bool,
    /// An import declaration: additions from both sides union.
    import: bool,
}

/// The byte range inside a container node, excluding its delimiters.
fn inner_range(container: tree_sitter::Node) -> (usize, usize) {
    let mut start = container.start_byte();
    let mut end = container.end_byte();
    let mut cursor = container.walk();
    let children: Vec<tree_sitter::Node> = container.children(&mut cursor).collect();
    if let Some(first) = children.first() {
        if matches!(first.kind(), "{" | "(" | "[") {
            start = first.end_byte();
        }
    }
    if let Some(last) = children.last() {
        if matches!(last.kind(), "}" | ")" | "]") && last.start_byte() >= start {
            end = last.start_byte();
        }
    }
    (start, end)
}

/// Split a container's children into keyed chunks. Each chunk owns the text
/// from the end of its predecessor, so trivia travels with the declaration that
/// follows it; whatever is left becomes a `__tail__` chunk.
fn chunk_container(lang: &str, src: &[u8], container: tree_sitter::Node) -> Vec<Chunk> {
    let (inner_start, inner_end) = inner_range(container);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut start = inner_start;
    let mut cursor = container.walk();
    for node in container.children(&mut cursor) {
        if node.start_byte() < inner_start || node.end_byte() > inner_end {
            continue;
        }
        if TRIVIA_TYPES.contains(&node.kind()) {
            continue;
        }
        let (mut key, is_anon) = node_key(node, src);
        let count = seen.entry(key.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            key = format!("{key}#{count}");
        }
        chunks.push(Chunk {
            key,
            text: String::from_utf8_lossy(&src[start..node.end_byte()]).into_owned(),
            anon: is_anon,
            import: import_kinds(lang).contains(&node.kind()),
        });
        start = node.end_byte();
    }
    let tail = String::from_utf8_lossy(&src[start..inner_end]).into_owned();
    if !tail.is_empty() {
        chunks.push(Chunk {
            key: "__tail__".into(),
            text: tail,
            anon: false,
            import: false,
        });
    }
    chunks
}

/// Top-level chunks of a whole revision, using an already-parsed tree.
fn root_chunks(lang: &str, text: &str, tree: &tree_sitter::Tree) -> Vec<Chunk> {
    chunk_container(lang, text.as_bytes(), tree.root_node())
}

/// Parse and chunk a revision from text, for callers outside the merge
/// pipeline that do not hold a tree.
fn parse_root_chunks(lang: &str, text: &str) -> Option<Vec<Chunk>> {
    let tree = parse_clean(lang, text)?;
    Some(root_chunks(lang, text, &tree))
}

/// Body-container kinds the recursion is willing to descend into.
fn body_container_kinds(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &[
            "declaration_list",
            "field_declaration_list",
            "enum_variant_list",
        ],
        "python" => &["block"],
        _ => &[
            "class_body",
            "statement_block",
            "object_type",
            "interface_body",
            "enum_body",
        ],
    }
}

/// The sole non-trivia declaration of a standalone chunk and the inner range of
/// its body. None when the chunk is not exactly one declaration with a body.
fn lone_decl_body(lang: &str, text: &str) -> Option<(tree_sitter::Tree, usize, usize)> {
    let tree = parse_clean(lang, text)?;
    let (start, end) = {
        let root = tree.root_node();
        let mut cursor = root.walk();
        let decls: Vec<tree_sitter::Node> = root
            .children(&mut cursor)
            .filter(|n| !TRIVIA_TYPES.contains(&n.kind()))
            .collect();
        if decls.len() != 1 {
            return None;
        }
        let body = decls[0].child_by_field_name("body")?;
        if !body_container_kinds(lang).contains(&body.kind()) {
            return None;
        }
        inner_range(body)
    };
    Some((tree, start, end))
}

/// Chunk the container whose inner range starts at `start`.
fn chunks_at(lang: &str, text: &str, tree: &tree_sitter::Tree, start: usize) -> Option<Vec<Chunk>> {
    let mut node = tree.root_node().descendant_for_byte_range(start, start);
    while let Some(n) = node {
        if inner_range(n).0 == start && n.child_count() > 0 {
            return Some(chunk_container(lang, text.as_bytes(), n));
        }
        node = n.parent();
    }
    None
}

/// Tier R: re-merge a declaration that changed on both sides by merging the
/// declarations inside its body. Conservative — the three sides must agree
/// byte-for-byte on everything outside the body, so a signature change never
/// composes silently with a body edit.
fn recurse_into_body(
    lang: &str,
    base: &str,
    left: &str,
    right: &str,
    depth: usize,
    stats: &mut BTreeMap<String, u32>,
) -> Option<String> {
    let (tb, bs, be) = lone_decl_body(lang, base)?;
    let (tl, ls, le) = lone_decl_body(lang, left)?;
    let (tr, rs, re) = lone_decl_body(lang, right)?;
    if base[..bs] != left[..ls] || base[..bs] != right[..rs] {
        return None;
    }
    if base[be..] != left[le..] || base[be..] != right[re..] {
        return None;
    }
    let cb = chunks_at(lang, base, &tb, bs)?;
    let cl = chunks_at(lang, left, &tl, ls)?;
    let cr = chunks_at(lang, right, &tr, rs)?;
    let inner = merge_chunks(lang, &cb, &cl, &cr, depth, stats)?;
    bump(stats, "tier_r_recursion");
    Some(format!("{}{}{}", &base[..bs], inner, &base[be..]))
}

/// Import chunk keys of one side.
fn import_keys(chunks: &[Chunk]) -> BTreeSet<&str> {
    chunks
        .iter()
        .filter(|c| c.import)
        .map(|c| c.key.as_str())
        .collect()
}

/// Anon-keyed chunks, which may not be reordered or unioned. Imports drop out
/// when both sides only ever added imports — that is what lets concurrent
/// import additions union instead of conflicting.
fn anon_keys(chunks: &[Chunk], union_imports: bool) -> Vec<&str> {
    let mut v: Vec<&str> = chunks
        .iter()
        .filter(|c| c.anon && !(union_imports && c.import))
        .map(|c| c.key.as_str())
        .collect();
    v.sort_unstable();
    v
}

/// Three-way merge of keyed chunk lists: matching keys resolve per side,
/// declarations dirty on both sides recurse into their bodies (Tier R), and the
/// result is ordered by base first, then by each side's insertion anchors.
fn merge_chunks(
    lang: &str,
    cb: &[Chunk],
    cl: &[Chunk],
    cr: &[Chunk],
    depth: usize,
    stats: &mut BTreeMap<String, u32>,
) -> Option<String> {
    let index = |chunks: &[Chunk]| -> Option<BTreeMap<String, String>> {
        let mut m = BTreeMap::new();
        for chunk in chunks {
            if m.insert(chunk.key.clone(), chunk.text.clone()).is_some() {
                return None;
            }
        }
        Some(m)
    };
    let mb = index(cb)?;
    let ml = index(cl)?;
    let mr = index(cr)?;

    let (ib, il, ir) = (import_keys(cb), import_keys(cl), import_keys(cr));
    let union_imports = ib.is_subset(&il) && ib.is_subset(&ir);
    let (ab, al, ar) = (
        anon_keys(cb, union_imports),
        anon_keys(cl, union_imports),
        anon_keys(cr, union_imports),
    );
    if ab != al && ab != ar {
        return None;
    }
    if union_imports && (il != ib || ir != ib) {
        bump(stats, "import_union");
    }

    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    let mut all_keys: Vec<&String> = mb.keys().chain(ml.keys()).chain(mr.keys()).collect();
    all_keys.sort_unstable();
    all_keys.dedup();
    for key in all_keys {
        let (b, l, r) = (mb.get(key), ml.get(key), mr.get(key));
        if l == r {
            if let Some(l) = l {
                merged.insert(key.clone(), l.clone());
            }
            continue;
        }
        if b == l {
            if let Some(r) = r {
                merged.insert(key.clone(), r.clone());
            }
            continue;
        }
        if b == r {
            if let Some(l) = l {
                merged.insert(key.clone(), l.clone());
            }
            continue;
        }
        let (Some(b), Some(l), Some(r)) = (b, l, r) else {
            return None;
        };
        if depth >= MAX_RECURSION {
            return None;
        }
        let inner = recurse_into_body(lang, b, l, r, depth + 1, stats)?;
        merged.insert(key.clone(), inner);
    }

    let mut order: Vec<String> = Vec::new();
    for chunk in cb {
        if merged.contains_key(&chunk.key) && !order.contains(&chunk.key) {
            order.push(chunk.key.clone());
        }
    }
    for side in [cl, cr] {
        let keys: Vec<&String> = side.iter().map(|c| &c.key).collect();
        for (i, key) in keys.iter().enumerate() {
            if merged.contains_key(*key) && !order.contains(key) {
                let anchor = keys[..i].iter().rev().find(|p| order.contains(**p));
                match anchor {
                    None => order.insert(0, (*key).clone()),
                    Some(anchor) => {
                        let pos = order.iter().position(|o| o == *anchor).unwrap();
                        order.insert(pos + 1, (*key).clone());
                    }
                }
            }
        }
    }
    if let Some(pos) = order.iter().position(|k| k == "__tail__") {
        let tail = order.remove(pos);
        order.push(tail);
    }
    Some(order.iter().map(|k| merged[k].clone()).collect::<String>())
}

/// Declaration-level merge over three already-parsed revisions.
fn decl_merge(lang: &str, revs: &Revisions, stats: &mut BTreeMap<String, u32>) -> Option<String> {
    let cb = root_chunks(lang, revs.base.0, &revs.base.1);
    let cl = root_chunks(lang, revs.left.0, &revs.left.1);
    let cr = root_chunks(lang, revs.right.0, &revs.right.1);
    let text = merge_chunks(lang, &cb, &cl, &cr, 0, stats)?;
    bump(stats, "decl_merge");
    Some(text)
}

/// Find the unique top-level decl chunk named `name`; returns its index in
/// the chunk list. None when absent, duplicated, or the file does not chunk.
fn named_chunk_index(chunks: &[Chunk], name: &str) -> Option<usize> {
    let mut found = None;
    for (i, chunk) in chunks.iter().enumerate() {
        let base = chunk.key.strip_prefix("export+").unwrap_or(&chunk.key);
        if base.split_once(':').is_some_and(|(_, n)| n == name) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found
}

/// Split `text` into the unique top-level decl chunk named `name` and the
/// remainder of the file with that chunk removed. Conservative: None on any
/// ambiguity (parse error, duplicate names, name absent).
pub fn extract_named_chunk(lang: &str, text: &str, name: &str) -> Option<(String, String)> {
    let chunks = parse_root_chunks(lang, text)?;
    let idx = named_chunk_index(&chunks, name)?;
    let chunk = chunks[idx].text.clone();
    let rest: String = chunks
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, c)| c.text.as_str())
        .collect();
    Some((chunk, rest))
}

/// Replace the unique top-level decl chunk named `name` in `text` with
/// `new_chunk`, preserving surrounding chunks. None on any ambiguity.
pub fn replace_named_chunk(lang: &str, text: &str, name: &str, new_chunk: &str) -> Option<String> {
    let chunks = parse_root_chunks(lang, text)?;
    let idx = named_chunk_index(&chunks, name)?;
    Some(
        chunks
            .iter()
            .enumerate()
            .map(|(i, c)| if i == idx { new_chunk } else { c.text.as_str() })
            .collect(),
    )
}

/// Rename the top-level declaration named `old` to `new` in `text`, touching
/// only the declaration's own name token. Returns None if it is not found.
fn rename_top_decl(lang: &str, text: &str, old: &str, new: &str) -> Option<String> {
    let tree = crate::parsers::parse(lang, text)?;
    let src = text.as_bytes();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for node in root.children(&mut cursor) {
        let target = if node.kind() == "export_statement" {
            node.child_by_field_name("declaration").unwrap_or(node)
        } else {
            node
        };
        if let Some(name_node) = target.child_by_field_name("name") {
            if &src[name_node.byte_range()] == old.as_bytes() {
                let mut out = text.to_string();
                out.replace_range(name_node.byte_range(), new);
                return Some(out);
            }
        }
    }
    None
}

/// Identity-aware declaration merge: uses the typed rename ops (base name ->
/// new name, per side) to match declarations across sides by identity rather
/// than by current name, so a rename on one side composes with a body edit on
/// the other. Conservative — returns None (falling back to a conflict) on
/// anything it cannot compose with certainty.
fn decl_merge_idaware(
    lang: &str,
    revs: &Revisions,
    left_ren: &BTreeMap<String, String>,
    right_ren: &BTreeMap<String, String>,
    stats: &mut BTreeMap<String, u32>,
) -> Option<String> {
    let cb = root_chunks(lang, revs.base.0, &revs.base.1);
    let cl = root_chunks(lang, revs.left.0, &revs.left.1);
    let cr = root_chunks(lang, revs.right.0, &revs.right.1);
    let mb: BTreeMap<&str, &str> = cb
        .iter()
        .map(|c| (c.key.as_str(), c.text.as_str()))
        .collect();
    if mb.len() != cb.len() {
        return None;
    }

    let l_rev: BTreeMap<&str, &str> = left_ren
        .iter()
        .map(|(b, n)| (n.as_str(), b.as_str()))
        .collect();
    let r_rev: BTreeMap<&str, &str> = right_ren
        .iter()
        .map(|(b, n)| (n.as_str(), b.as_str()))
        .collect();
    let canon = |key: &str, rev: &BTreeMap<&str, &str>| -> String {
        match key.split_once(':') {
            Some((kind, name)) => match rev.get(name) {
                Some(base_name) => format!("{kind}:{base_name}"),
                None => key.to_string(),
            },
            None => key.to_string(),
        }
    };
    let cl_canon: BTreeMap<String, &str> = cl
        .iter()
        .map(|c| (canon(&c.key, &l_rev), c.text.as_str()))
        .collect();
    let cr_canon: BTreeMap<String, &str> = cr
        .iter()
        .map(|c| (canon(&c.key, &r_rev), c.text.as_str()))
        .collect();
    if cl_canon.len() != cl.len() || cr_canon.len() != cr.len() {
        return None;
    }

    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    let mut all: Vec<String> = mb
        .keys()
        .map(|k| k.to_string())
        .chain(cl_canon.keys().cloned())
        .chain(cr_canon.keys().cloned())
        .collect();
    all.sort_unstable();
    all.dedup();

    for key in &all {
        let b = mb.get(key.as_str()).copied();
        let l = cl_canon.get(key).copied();
        let r = cr_canon.get(key).copied();
        if l == r {
            if let Some(l) = l {
                merged.insert(key.clone(), l.to_string());
            }
            continue;
        }
        if b == l {
            if let Some(r) = r {
                merged.insert(key.clone(), r.to_string());
            }
            continue;
        }
        if b == r {
            if let Some(l) = l {
                merged.insert(key.clone(), l.to_string());
            }
            continue;
        }
        // Both sides diverge from base: only the rename+body compose is safe.
        let (Some(bt), Some(lt), Some(rt)) = (b, l, r) else {
            return None;
        };
        let base_name = key.split_once(':').map(|(_, n)| n.to_string())?;
        let l_new = left_ren.get(&base_name);
        let r_new = right_ren.get(&base_name);
        if let Some(l_new) = l_new {
            if r_new.is_none()
                && rename_top_decl(lang, lt, l_new, &base_name).as_deref() == Some(bt)
                && rt != bt
            {
                let composed = rename_top_decl(lang, rt, &base_name, l_new)?;
                merged.insert(key.clone(), composed);
                continue;
            }
        }
        if let Some(r_new) = r_new {
            if l_new.is_none()
                && rename_top_decl(lang, rt, r_new, &base_name).as_deref() == Some(bt)
                && lt != bt
            {
                let composed = rename_top_decl(lang, lt, &base_name, r_new)?;
                merged.insert(key.clone(), composed);
                continue;
            }
        }
        return None;
    }

    let mut order: Vec<String> = Vec::new();
    for chunk in &cb {
        if merged.contains_key(&chunk.key) && !order.contains(&chunk.key) {
            order.push(chunk.key.clone());
        }
    }
    for canon_keys in [&cl_canon, &cr_canon] {
        for k in canon_keys.keys() {
            if merged.contains_key(k) && !order.contains(k) {
                order.push(k.clone());
            }
        }
    }
    if let Some(pos) = order.iter().position(|k| k == "__tail__") {
        let tail = order.remove(pos);
        order.push(tail);
    }
    bump(stats, "idaware_decl_merge");
    Some(order.iter().map(|k| merged[k].clone()).collect::<String>())
}

/// The three revisions of one file, each parsed exactly once per merge.
struct Revisions<'a> {
    base: (&'a str, tree_sitter::Tree),
    left: (&'a str, tree_sitter::Tree),
    right: (&'a str, tree_sitter::Tree),
}

impl<'a> Revisions<'a> {
    /// Parse all three revisions, or None when any of them fails to parse
    /// cleanly — the text-merge fallback path.
    fn parse(lang: &str, base: &'a str, left: &'a str, right: &'a str) -> Option<Self> {
        Some(Revisions {
            base: (base, parse_clean(lang, base)?),
            left: (left, parse_clean(lang, left)?),
            right: (right, parse_clean(lang, right)?),
        })
    }
}

/// Identity-aware merge entry point: run the normal pipeline first, then — only
/// if it did not resolve — use the typed rename ops to compose a rename on one
/// side with a body edit on the other. Falls back to the normal outcome on any
/// uncertainty, so it can only ever turn a conflict into a clean merge.
pub fn merge_file_idaware(
    lang: Option<&str>,
    base: &str,
    left: &str,
    right: &str,
    left_ren: &BTreeMap<String, String>,
    right_ren: &BTreeMap<String, String>,
) -> Result<MergeOutcome> {
    let (base_out, revs) = merge_inner(lang, base, left, right)?;
    if base_out.status == MergeStatus::Clean {
        return Ok(base_out);
    }
    let Some(lang) = lang else {
        return Ok(base_out);
    };
    if left_ren.is_empty() && right_ren.is_empty() {
        return Ok(base_out);
    }
    let Some(revs) = revs else {
        return Ok(base_out);
    };
    let mut stats = base_out.strategies.clone();
    if let Some(text) = decl_merge_idaware(lang, &revs, left_ren, right_ren, &mut stats) {
        if parses_clean(lang, &text) {
            return Ok(MergeOutcome {
                status: MergeStatus::Clean,
                text,
                strategies: stats,
            });
        }
    }
    Ok(base_out)
}

/// Full pipeline: line merge fast path, hunk strategies, then decl merge.
pub fn merge_file(lang: Option<&str>, base: &str, left: &str, right: &str) -> Result<MergeOutcome> {
    Ok(merge_inner(lang, base, left, right)?.0)
}

/// The pipeline, also handing back the parsed revisions so the identity-aware
/// path never re-parses them.
fn merge_inner<'a>(
    lang: Option<&str>,
    base: &'a str,
    left: &'a str,
    right: &'a str,
) -> Result<(MergeOutcome, Option<Revisions<'a>>)> {
    let mut stats = BTreeMap::new();
    let (conflicts, marked) = line_merge(base, left, right)?;
    if conflicts == 0 {
        bump(&mut stats, "line_clean");
        return Ok((
            MergeOutcome {
                status: MergeStatus::Clean,
                text: marked,
                strategies: stats,
            },
            None,
        ));
    }
    let Some(lang) = lang else {
        return Ok((
            MergeOutcome {
                status: MergeStatus::Conflict,
                text: marked,
                strategies: stats,
            },
            None,
        ));
    };
    let Some(revs) = Revisions::parse(lang, base, left, right) else {
        return Ok((
            MergeOutcome {
                status: MergeStatus::ParseFallback,
                text: marked,
                strategies: stats,
            },
            None,
        ));
    };
    let doc = parse_markers(&marked);
    let mut resolved: Vec<Segment> = Vec::new();
    let mut unresolved = 0usize;
    for (idx, seg) in doc.iter().enumerate() {
        match seg {
            Segment::Conflict(hunk) => {
                match resolve_hunk(hunk, &doc, idx, &revs.base.1, lang, &mut stats) {
                    Some(lines) => resolved.push(Segment::Context(lines)),
                    None => {
                        unresolved += 1;
                        resolved.push(seg.clone());
                    }
                }
            }
            Segment::Context(_) => resolved.push(seg.clone()),
        }
    }
    if unresolved == 0 {
        let text = render(&resolved, base.ends_with('\n'))?;
        if parses_clean(lang, &text) {
            return Ok((
                MergeOutcome {
                    status: MergeStatus::Clean,
                    text,
                    strategies: stats,
                },
                Some(revs),
            ));
        }
        bump(&mut stats, "downgraded_parse");
    }
    if let Some(text) = decl_merge(lang, &revs, &mut stats) {
        if parses_clean(lang, &text) {
            return Ok((
                MergeOutcome {
                    status: MergeStatus::Clean,
                    text,
                    strategies: stats,
                },
                Some(revs),
            ));
        }
    }
    bump(&mut stats, "true_conflict");
    Ok((
        MergeOutcome {
            status: MergeStatus::Conflict,
            text: marked,
            strategies: stats,
        },
        Some(revs),
    ))
}
