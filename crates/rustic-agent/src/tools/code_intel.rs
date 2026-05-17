//! Code-intelligence tools backed by the workspace symbol index and tree-sitter.
//! Read-only; share one index+parser pool via `WorkspaceServices`.
//! Tools: `find_symbol`, `goto_definition`, `find_references`, `outline`, `call_sites`.

use super::{ToolContext, ToolOutput};
use crate::index::{SymbolEntry, SymbolKind};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use streaming_iterator::StreamingIterator;

/// Default result cap. The agent can ask for more via `limit`, but we keep
/// the default low so a clumsy `find_references("get")` doesn't drown the
/// context window.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "find_symbol".into(),
            description: "Find declarations of a symbol across the project by exact name. \
                          Returns file/line/column plus declaration kind (function, method, \
                          class, struct, enum, trait, interface, type, module, constant, \
                          variable, macro). Use this BEFORE `read_file` when looking for a \
                          known identifier — it's faster than grep and tells you the \
                          declaration kind. Falls back to a case-insensitive substring search \
                          when the exact name has no hits."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Symbol name to look up (case-sensitive for exact match)."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional kind filter: function, method, class, struct, \
                                         enum, trait, interface, type, module, variable, constant, macro."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results to return (default 50, max 500)."
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDef {
            name: "goto_definition".into(),
            description: "Resolve the identifier at a specific file/line/column to its \
                          declaration site(s) in the project. NAME-RESOLUTION-ONLY — it does \
                          not understand types, so a method call returns every declaration \
                          of that method name across the project. Use when you have a use site \
                          and want to jump to the source of truth."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the file containing the use site (project-relative)."
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-indexed line number where the identifier appears."
                    },
                    "col": {
                        "type": "integer",
                        "description": "1-indexed column where the identifier starts."
                    }
                },
                "required": ["file", "line", "col"]
            }),
        },
        ToolDef {
            name: "find_references".into(),
            description: "Find every occurrence of an identifier with the given name across the \
                          project. NAME-MATCH-ONLY — does not differentiate between distinct \
                          identifiers that happen to share a name. Skips identifiers inside \
                          comments and string literals (via tree-sitter). Results capped at 50 \
                          by default; pass `limit` to widen."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Exact identifier text to search for."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results (default 50, max 500)."
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDef {
            name: "outline".into(),
            description: "List the declarations in one file, in source order: functions, methods, \
                          classes, structs, enums, traits, interfaces, type aliases, modules, \
                          top-level constants. Useful for getting your bearings in an unfamiliar \
                          file without reading the whole thing."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the file (project-relative or absolute)."
                    }
                },
                "required": ["file"]
            }),
        },
        ToolDef {
            name: "call_sites".into(),
            description: "Find every call expression whose callee identifier matches `name`. \
                          Like `find_references` but filters to *uses as a callable* — function \
                          calls, method calls, macro invocations. Faster signal than \
                          `find_references` when you specifically want to see who calls something."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Callee identifier to search for."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results (default 50, max 500)."
                    }
                },
                "required": ["name"]
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "Permission denied: read not allowed".into(),
            is_error: true, attachments: Vec::new() });
    }

    // Lazily kick off the project's symbol-index build. First caller wins;
    // subsequent calls return immediately.
    context.workspace_services.ensure_index_build_started();

    match name {
        "find_symbol" => execute_find_symbol(params, context).await,
        "goto_definition" => execute_goto_definition(params, context).await,
        "find_references" => execute_find_references(params, context).await,
        "outline" => execute_outline(params, context).await,
        "call_sites" => execute_call_sites(params, context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown code-intel tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn execute_find_symbol(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let name = match params["name"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(ToolOutput {
                content: "`name` is required and must be a non-empty string".into(),
                is_error: true, attachments: Vec::new() })
        }
    };
    let kind = params["kind"].as_str().and_then(SymbolKind::from_str);
    let limit = resolve_limit(&params);

    let index = context.workspace_services.symbol_index();
    let mut hits = index.find(name, kind, limit);
    let used_substring = hits.is_empty() && {
        hits = index.find_substring(name, kind, limit);
        !hits.is_empty()
    };

    let status_tag = index_status_tag(index.status());
    if hits.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "No symbols found for `{}`{}.{}",
                name,
                kind.map(|k| format!(" (kind={})", k.as_str())).unwrap_or_default(),
                status_tag,
            ),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let mut out = String::new();
    if used_substring {
        out.push_str("(exact match miss — showing substring matches)\n");
    }
    out.push_str(&render_entries(&hits, &context.project_root));
    out.push_str(&status_tag);
    Ok(ToolOutput {
        content: out,
        is_error: false, attachments: Vec::new() })
}

async fn execute_goto_definition(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let file = match params["file"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(ToolOutput {
                content: "`file` is required".into(),
                is_error: true, attachments: Vec::new() })
        }
    };
    let line = match params["line"].as_u64() {
        Some(n) if n >= 1 => n as usize,
        _ => {
            return Ok(ToolOutput {
                content: "`line` is required and must be >= 1".into(),
                is_error: true, attachments: Vec::new() })
        }
    };
    let col = match params["col"].as_u64() {
        Some(n) if n >= 1 => n as usize,
        _ => {
            return Ok(ToolOutput {
                content: "`col` is required and must be >= 1".into(),
                is_error: true, attachments: Vec::new() })
        }
    };

    let abs = resolve_path(&context.project_root, file);
    let bytes = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("Could not read `{}`: {}", file, e),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };
    let mtime = std::fs::metadata(&abs)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let ts = context.workspace_services.tree_sitter();
    let tree = match ts.parse(&abs, mtime, &bytes) {
        Some(t) => t,
        None => {
            return Ok(ToolOutput {
                content: format!(
                    "Cannot parse `{}` — no tree-sitter grammar registered for that extension.",
                    file
                ),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };

    // The model talks in 1-indexed (line, col); tree-sitter uses 0-indexed.
    let point = tree_sitter::Point::new(line - 1, col - 1);
    let mut node = tree.root_node().descendant_for_point_range(point, point);
    while let Some(n) = node {
        if is_identifier_kind(n.kind()) {
            let name = match n.utf8_text(&bytes) {
                Ok(s) => s.trim().to_string(),
                Err(_) => break,
            };
            let hits = context.workspace_services.symbol_index().find(&name, None, MAX_LIMIT);
            let status_tag = index_status_tag(context.workspace_services.symbol_index().status());
            if hits.is_empty() {
                return Ok(ToolOutput {
                    content: format!(
                        "No declarations indexed for identifier `{}`.{}",
                        name, status_tag
                    ),
                    is_error: false,
                    attachments: Vec::new(),
                });
            }
            let mut out = format!("Resolved `{}` to {} declaration(s):\n", name, hits.len());
            out.push_str(&render_entries(&hits, &context.project_root));
            out.push_str(&status_tag);
            return Ok(ToolOutput {
                content: out,
                is_error: false, attachments: Vec::new() });
        }
        node = n.parent();
    }

    Ok(ToolOutput {
        content: format!(
            "No identifier at {}:{}:{} — the cursor is on punctuation, whitespace, or a non-identifier token.",
            file, line, col
        ),
        is_error: false,
        attachments: Vec::new(),
    })
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "simple_identifier"
            | "shorthand_property_identifier"
            | "name"
            | "constant"
    )
}

async fn execute_find_references(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let name = match params["name"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(ToolOutput {
                content: "`name` is required".into(),
                is_error: true, attachments: Vec::new() })
        }
    };
    let limit = resolve_limit(&params);
    let hits = node_search(context, name, limit, |_node| true);
    let status_tag = index_status_tag(context.workspace_services.symbol_index().status());
    if hits.is_empty() {
        return Ok(ToolOutput {
            content: format!("No references to `{}` found.{}", name, status_tag),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let mut out = format!("Found {} reference(s) to `{}`:\n", hits.len(), name);
    out.push_str(&render_locations(&hits, &context.project_root));
    out.push_str("\n(name-match only — distinct identifiers with the same name are conflated)");
    out.push_str(&status_tag);
    Ok(ToolOutput {
        content: out,
        is_error: false, attachments: Vec::new() })
}

async fn execute_outline(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let file = match params["file"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(ToolOutput {
                content: "`file` is required".into(),
                is_error: true, attachments: Vec::new() })
        }
    };
    let abs = resolve_path(&context.project_root, file);
    if !abs.exists() {
        return Ok(ToolOutput {
            content: format!("File not found: {}", file),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    // Gate the refresh on mtime — reparsing burns CPU for no gain when the stored entries are still current.
    let needs_refresh = match std::fs::metadata(&abs).and_then(|m| m.modified()) {
        Ok(current_mtime) => !context
            .workspace_services
            .symbol_index()
            .is_file_fresh(&abs, current_mtime),
        Err(_) => true,
    };
    if needs_refresh {
        let _ = crate::index::refresh_file(
            &abs,
            context.workspace_services.tree_sitter(),
            context.workspace_services.symbol_index(),
        );
    }

    let entries = context.workspace_services.symbol_index().entries_in_file(&abs);
    if entries.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "No declarations found in `{}` (file may be in an unsupported language or contain no top-level items).",
                file
            ),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let project_rel = to_project_relative(&abs, &context.project_root);
    let mut out = format!("Outline of `{}` ({} declaration(s)):\n", project_rel, entries.len());
    for entry in entries {
        match &entry.scope {
            Some(scope) => out.push_str(&format!(
                "  {:>5}: {} {} (in {})\n",
                entry.line,
                entry.kind.as_str(),
                entry.name,
                scope
            )),
            None => out.push_str(&format!(
                "  {:>5}: {} {}\n",
                entry.line,
                entry.kind.as_str(),
                entry.name
            )),
        }
    }
    Ok(ToolOutput {
        content: out,
        is_error: false, attachments: Vec::new() })
}

async fn execute_call_sites(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let name = match params["name"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(ToolOutput {
                content: "`name` is required".into(),
                is_error: true, attachments: Vec::new() })
        }
    };
    let limit = resolve_limit(&params);
    let hits = node_search(context, name, limit, is_call_site_node);
    let status_tag = index_status_tag(context.workspace_services.symbol_index().status());
    if hits.is_empty() {
        return Ok(ToolOutput {
            content: format!("No call sites for `{}` found.{}", name, status_tag),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let mut out = format!("Found {} call site(s) for `{}`:\n", hits.len(), name);
    out.push_str(&render_locations(&hits, &context.project_root));
    out.push_str(&status_tag);
    Ok(ToolOutput {
        content: out,
        is_error: false, attachments: Vec::new() })
}

fn is_call_parent_kind(kind: &str) -> bool {
    matches!(
        kind,
        "call_expression"
            | "call"
            | "function_call"
            | "method_invocation"
            | "method_call"
            | "method_call_expression"
            | "invocation_expression"
            | "macro_invocation"
            | "new_expression"
    )
}

fn is_field_access_kind(kind: &str) -> bool {
    matches!(
        kind,
        "field_expression"
            | "member_expression"
            | "member_access_expression"
            | "attribute"
            | "selector_expression"
            | "scoped_identifier"
            | "scoped_type_identifier"
            | "qualified_identifier"
    )
}

/// True when the identifier node is the callee of a call expression.
/// Handles both bare `foo()` (parent is call) and method-style `obj.foo()`
/// (identifier under a field-access node whose parent is a call).
fn is_call_site_node(node: &tree_sitter::Node) -> bool {
    let Some(parent) = node.parent() else { return false };
    let parent_kind = parent.kind();
    if is_call_parent_kind(parent_kind) {
        return true;
    }
    if !is_field_access_kind(parent_kind) {
        return false;
    }
    // Require this node to be the RHS (name slot) of the field-access, not the receiver.
    // tree-sitter 0.26 takes child index as u32.
    let is_rhs = {
        let n = *node;
        let last_named = parent.child_by_field_name("field")
            .or_else(|| parent.child_by_field_name("name"))
            .or_else(|| parent.child_by_field_name("property"))
            .or_else(|| parent.child_by_field_name("right"));
        if let Some(rhs) = last_named {
            rhs == n
        } else {
            let count = parent.named_child_count();
            if count == 0 {
                false
            } else {
                parent.named_child((count - 1) as u32) == Some(n)
            }
        }
    };
    if !is_rhs {
        return false;
    }
    let Some(grandparent) = parent.parent() else { return false };
    is_call_parent_kind(grandparent.kind())
}

/// Found-location record for the reference / call-site tools.
struct Location {
    file: PathBuf,
    line: u32,
    col: u32,
}

fn node_search(
    context: &ToolContext,
    name: &str,
    limit: usize,
    node_filter: impl Fn(&tree_sitter::Node) -> bool,
) -> Vec<Location> {
    let mut hits = Vec::new();
    let walker = ignore::WalkBuilder::new(&context.project_root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .build();
    for entry in walker.flatten() {
        if hits.len() >= limit {
            break;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(_lang) = rustic_treesitter::language_for_path(path) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(path) else { continue };
        if bytes.len() > 2 * 1024 * 1024 {
            continue;
        }
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let Some(tree) =
            context.workspace_services.tree_sitter().parse(path, mtime, &bytes)
        else {
            continue;
        };
        collect_identifier_matches(
            &tree,
            &bytes,
            path,
            name,
            limit,
            &node_filter,
            &mut hits,
        );
    }
    hits
}

fn collect_identifier_matches(
    tree: &tree_sitter::Tree,
    bytes: &[u8],
    path: &Path,
    target: &str,
    limit: usize,
    node_filter: &impl Fn(&tree_sitter::Node) -> bool,
    out: &mut Vec<Location>,
) {
    let language = tree.language();
    let query_src = "
        [(identifier)
         (type_identifier)
         (field_identifier)
         (property_identifier)
         (simple_identifier)
         (shorthand_property_identifier)
         (name)] @id
    ";
    // Some grammars don't have every node kind above; fall back to (identifier) rather than failing.
    let query = match tree_sitter::Query::new(&language, query_src) {
        Ok(q) => q,
        Err(_) => {
            // Try just the universal `(identifier)` form.
            match tree_sitter::Query::new(&language, "(identifier) @id") {
                Ok(q) => q,
                Err(_) => return,
            }
        }
    };
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if out.len() >= limit {
                return;
            }
            let node = cap.node;
            let Ok(text) = node.utf8_text(bytes) else { continue };
            if text != target {
                continue;
            }
            if !node_filter(&node) {
                continue;
            }
            let start = node.start_position();
            out.push(Location {
                file: path.to_path_buf(),
                line: (start.row as u32).saturating_add(1),
                col: (start.column as u32).saturating_add(1),
            });
        }
    }
}

fn render_entries(entries: &[SymbolEntry], project_root: &Path) -> String {
    let mut out = String::new();
    for entry in entries {
        let rel = to_project_relative(&entry.file, project_root);
        out.push_str(&entry.render_line(&rel));
        out.push('\n');
    }
    out
}

fn render_locations(locations: &[Location], project_root: &Path) -> String {
    let mut out = String::new();
    for loc in locations {
        let rel = to_project_relative(&loc.file, project_root);
        out.push_str(&format!("  {}:{}:{}\n", rel, loc.line, loc.col));
    }
    out
}

fn to_project_relative(path: &Path, project_root: &Path) -> String {
    path.strip_prefix(project_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn resolve_path(project_root: &Path, file: &str) -> PathBuf {
    let p = PathBuf::from(file);
    if p.is_absolute() {
        p
    } else {
        project_root.join(p)
    }
}

fn resolve_limit(params: &Value) -> usize {
    params["limit"]
        .as_u64()
        .map(|n| (n as usize).clamp(1, MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
}

fn index_status_tag(status: crate::index::IndexStatus) -> String {
    use crate::index::IndexStatus;
    match status {
        IndexStatus::Building => "\n[index still building — results may be incomplete]".to_string(),
        IndexStatus::Failed => "\n[index build failed — only partial results available]".to_string(),
        IndexStatus::NotStarted | IndexStatus::Ready => String::new(),
    }
}

#[cfg(test)]
mod l1_call_site_tests {
    use super::is_call_site_node;
    use rustic_treesitter::WorkspaceTreesitter;
    use std::sync::Arc;
    use std::time::SystemTime;

    fn first_identifier_is_call_site(language_name: &str, source: &str, target: &str) -> bool {
        let ts = Arc::new(WorkspaceTreesitter::new());
        let path = std::path::PathBuf::from(format!("test.{}", extension_for(language_name)));
        let tree = ts
            .parse(&path, SystemTime::UNIX_EPOCH, source.as_bytes())
            .expect("parser available + parse succeeds");
        let target_text = target.as_bytes();
        // Walk the whole tree looking for the first identifier-class node
        // whose bytes equal `target`.
        let mut cursor = tree.walk();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if is_identifier_like_kind(node.kind()) {
                let start = node.start_byte();
                let end = node.end_byte();
                if &source.as_bytes()[start..end] == target_text {
                    return is_call_site_node(&node);
                }
            }
            // Push children in reverse for left-to-right traversal; tree-sitter 0.26 uses u32 index.
            let count = node.named_child_count();
            for i in (0..count).rev() {
                if let Some(c) = node.named_child(i as u32) {
                    stack.push(c);
                }
            }
            let _ = cursor.goto_first_child();
        }
        panic!(
            "no identifier `{}` found in {} source — grammar may have changed",
            target, language_name
        );
    }

    fn is_identifier_like_kind(kind: &str) -> bool {
        matches!(
            kind,
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "property_identifier"
                | "simple_identifier"
                | "shorthand_property_identifier"
                | "name"
        )
    }

    fn extension_for(language_name: &str) -> &'static str {
        match language_name {
            "rust" => "rs",
            "javascript" => "js",
            "typescript" => "ts",
            "python" => "py",
            "go" => "go",
            other => panic!("unsupported language in test: {}", other),
        }
    }

    #[test]
    fn bare_call_is_recognized_rust() {
        assert!(first_identifier_is_call_site("rust", "fn m() { foo(); }", "foo"));
    }

    #[test]
    fn bare_call_is_recognized_python() {
        assert!(first_identifier_is_call_site("python", "foo()", "foo"));
    }

    #[test]
    fn bare_call_is_recognized_go() {
        assert!(first_identifier_is_call_site(
            "go",
            "package m\nfunc m() { foo() }",
            "foo",
        ));
    }

    #[test]
    fn method_call_is_recognized_rust() {
        assert!(first_identifier_is_call_site(
            "rust",
            "fn m() { obj.method(); }",
            "method",
        ));
    }

    #[test]
    fn method_call_is_recognized_javascript() {
        assert!(first_identifier_is_call_site(
            "javascript",
            "obj.method();",
            "method",
        ));
    }

    #[test]
    fn method_call_is_recognized_typescript() {
        assert!(first_identifier_is_call_site(
            "typescript",
            "obj.method();",
            "method",
        ));
    }

    #[test]
    fn method_call_is_recognized_python() {
        assert!(first_identifier_is_call_site(
            "python",
            "self.foo()",
            "foo",
        ));
    }

    #[test]
    fn method_call_is_recognized_go() {
        assert!(first_identifier_is_call_site(
            "go",
            "package m\nfunc m() { r.Read() }",
            "Read",
        ));
    }

    #[test]
    fn field_read_is_not_a_call_site_rust() {
        assert!(!first_identifier_is_call_site(
            "rust",
            "fn m() { let _ = obj.method; }",
            "method",
        ));
    }

    #[test]
    fn field_assignment_is_not_a_call_site_javascript() {
        assert!(!first_identifier_is_call_site(
            "javascript",
            "obj.method = 1;",
            "method",
        ));
    }

    #[test]
    fn attribute_read_is_not_a_call_site_python() {
        assert!(!first_identifier_is_call_site(
            "python",
            "x = self.foo",
            "foo",
        ));
    }

    #[test]
    fn receiver_identifier_is_not_the_callee() {
        assert!(!first_identifier_is_call_site(
            "rust",
            "fn m() { obj.method(); }",
            "obj",
        ));
    }
}
