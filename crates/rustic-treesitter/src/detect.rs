//! File-extension → tree-sitter language name mapping.
//!
//! Mirrors `rustic_core::syntax::LanguageRegistry::get_language(name)` so a
//! `path → name → Language` lookup gives the same grammar the editor's
//! syntax highlighter is using.

use std::path::Path;

/// Return the language name (matching rustic-core's `LanguageRegistry`) for
/// the file at `path`, or `None` if the extension isn't recognized.
///
/// The match is case-insensitive on the extension. Files without an
/// extension return `None` — the symbol indexer should skip them rather
/// than guess.
pub fn language_for_path(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    language_for_extension(&ext)
}

/// Same as `language_for_path` but takes a bare extension string (no leading
/// dot). Useful when the caller already split the extension off.
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "rs" => Some("rust"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        // tree-sitter-javascript handles JSX too, per rustic-core's mapping.
        "jsx" => Some("javascript"),
        "py" | "pyi" => Some("python"),
        "go" => Some("go"),
        "java" => Some("java"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Some("cpp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "lua" => Some("lua"),
        "scala" | "sc" => Some("scala"),
        "swift" => Some("swift"),
        "dart" => Some("dart"),
        "kt" | "kts" => Some("kotlin"),
        "cs" => Some("csharp"),
        "zig" => Some("zig"),
        "ex" | "exs" => Some("elixir"),
        "r" => Some("r"),
        "svelte" => Some("svelte"),
        "nix" => Some("nix"),
        "hs" => Some("haskell"),
        "sh" | "bash" => Some("bash"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "sql" => Some("sql"),
        "md" | "markdown" => Some("markdown"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_rust_from_extension() {
        assert_eq!(language_for_path(&PathBuf::from("foo.rs")), Some("rust"));
        assert_eq!(
            language_for_path(&PathBuf::from("/abs/path/foo.rs")),
            Some("rust")
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            language_for_path(&PathBuf::from("App.TS")),
            Some("typescript")
        );
    }

    #[test]
    fn separates_tsx_from_typescript() {
        assert_eq!(language_for_path(&PathBuf::from("App.tsx")), Some("tsx"));
        assert_eq!(
            language_for_path(&PathBuf::from("App.ts")),
            Some("typescript")
        );
    }

    #[test]
    fn unknown_extension_returns_none() {
        assert_eq!(language_for_path(&PathBuf::from("README")), None);
        assert_eq!(language_for_path(&PathBuf::from("foo.xyz")), None);
    }
}
