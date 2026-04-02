use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::edit::{Edit, EditGroup};

static NEXT_BUFFER_ID: AtomicU64 = AtomicU64::new(1);

pub type BufferId = u64;

pub fn next_buffer_id() -> BufferId {
    NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BufferInfo {
    pub id: BufferId,
    pub file_path: Option<String>,
    pub file_name: String,
    pub line_count: usize,
    pub language: Option<String>,
    pub is_modified: bool,
}

pub struct Buffer {
    pub id: BufferId,
    pub rope: Rope,
    pub file_path: Option<PathBuf>,
    pub is_modified: bool,
    pub language: Option<String>,
    pub undo_stack: Vec<EditGroup>,
    pub redo_stack: Vec<EditGroup>,
    last_edit_time: Option<std::time::Instant>,
}

impl Buffer {
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let rope = Rope::from_str(&content);
        let language = detect_language(path)
            .or_else(|| detect_language_from_content(&content));

        Ok(Self {
            id: next_buffer_id(),
            rope,
            file_path: Some(path.to_path_buf()),
            is_modified: false,
            language,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_time: None,
        })
    }

    pub fn from_string(content: &str) -> Self {
        Self {
            id: next_buffer_id(),
            rope: Rope::from_str(content),
            file_path: None,
            is_modified: false,
            language: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_time: None,
        }
    }

    pub fn info(&self) -> BufferInfo {
        let file_name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        BufferInfo {
            id: self.id,
            file_path: self.file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            file_name,
            line_count: self.rope.len_lines(),
            language: self.language.clone(),
            is_modified: self.is_modified,
        }
    }

    pub fn apply_edit(&mut self, edit: Edit) -> anyhow::Result<()> {
        let now = std::time::Instant::now();
        let should_group = self
            .last_edit_time
            .map(|t| now.duration_since(t).as_millis() < 300)
            .unwrap_or(false);

        // Apply the edit to the rope
        let char_start = self.rope.byte_to_char(edit.byte_offset);
        let char_end = self.rope.byte_to_char(edit.byte_offset + edit.old_text.len());

        if !edit.old_text.is_empty() {
            self.rope.remove(char_start..char_end);
        }
        if !edit.new_text.is_empty() {
            self.rope.insert(char_start, &edit.new_text);
        }

        // Push to undo stack
        if should_group {
            if let Some(group) = self.undo_stack.last_mut() {
                group.edits.push(edit);
            } else {
                self.undo_stack.push(EditGroup {
                    edits: vec![edit],
                });
            }
        } else {
            self.undo_stack.push(EditGroup {
                edits: vec![edit],
            });
        }

        self.redo_stack.clear();
        self.is_modified = true;
        self.last_edit_time = Some(now);

        Ok(())
    }

    pub fn undo(&mut self) -> Option<Vec<Edit>> {
        let group = self.undo_stack.pop()?;
        let mut inverse_edits = Vec::new();

        // Apply edits in reverse order
        for edit in group.edits.iter().rev() {
            let inverse = edit.inverse();
            let char_start = self.rope.byte_to_char(inverse.byte_offset);
            let char_end = self.rope.byte_to_char(inverse.byte_offset + inverse.old_text.len());

            if !inverse.old_text.is_empty() {
                self.rope.remove(char_start..char_end);
            }
            if !inverse.new_text.is_empty() {
                self.rope.insert(char_start, &inverse.new_text);
            }
            inverse_edits.push(inverse);
        }

        self.redo_stack.push(group);
        self.is_modified = !self.undo_stack.is_empty() || self.file_path.is_some();

        Some(inverse_edits)
    }

    pub fn redo(&mut self) -> Option<Vec<Edit>> {
        let group = self.redo_stack.pop()?;
        let mut applied_edits = Vec::new();

        for edit in &group.edits {
            let char_start = self.rope.byte_to_char(edit.byte_offset);
            let char_end = self.rope.byte_to_char(edit.byte_offset + edit.old_text.len());

            if !edit.old_text.is_empty() {
                self.rope.remove(char_start..char_end);
            }
            if !edit.new_text.is_empty() {
                self.rope.insert(char_start, &edit.new_text);
            }
            applied_edits.push(edit.clone());
        }

        self.undo_stack.push(group);
        self.is_modified = true;

        Some(applied_edits)
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        if let Some(ref path) = self.file_path {
            let content = self.rope.to_string();
            std::fs::write(path, content)?;
            self.is_modified = false;
            Ok(())
        } else {
            anyhow::bail!("No file path set for buffer")
        }
    }

    // Line access methods
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn get_line(&self, idx: usize) -> Option<String> {
        if idx >= self.rope.len_lines() {
            return None;
        }
        let line = self.rope.line(idx);
        // Strip trailing newline for display
        let s = line.to_string();
        Some(s.trim_end_matches('\n').trim_end_matches('\r').to_string())
    }

    pub fn get_lines(&self, start: usize, end: usize) -> Vec<String> {
        let end = end.min(self.rope.len_lines());
        (start..end)
            .filter_map(|i| self.get_line(i))
            .collect()
    }

    pub fn byte_offset_of_line(&self, line_idx: usize) -> usize {
        if line_idx >= self.rope.len_lines() {
            return self.rope.len_bytes();
        }
        self.rope.char_to_byte(self.rope.line_to_char(line_idx))
    }

    pub fn line_of_byte(&self, byte_offset: usize) -> usize {
        let char_idx = self.rope.byte_to_char(byte_offset.min(self.rope.len_bytes()));
        self.rope.char_to_line(char_idx)
    }
}

fn detect_language(path: &std::path::Path) -> Option<String> {
    // Check for special filenames first
    let file_name = path.file_name()?.to_str()?;
    let by_filename = match file_name {
        // Build systems & task runners
        "Makefile" | "makefile" | "GNUmakefile" => Some("bash"),
        "Justfile" | "justfile" => Some("bash"),
        "Taskfile.yml" | "Taskfile.yaml" => Some("yaml"),

        // Shell configs
        ".bashrc" | ".bash_profile" | ".bash_logout" | ".bash_aliases"
        | ".zshrc" | ".zprofile" | ".zshenv" | ".zlogin"
        | ".profile" | ".login" | ".cshrc" | ".tcshrc"
        | ".inputrc" | ".screenrc" | ".tmux.conf" => Some("bash"),

        // Lock files
        "Cargo.lock" | "poetry.lock" | "uv.lock" => Some("toml"),
        "composer.lock" | "Pipfile.lock" => Some("json"),
        "yarn.lock" | "bun.lock" | "pnpm-lock.yaml" => Some("yaml"),
        "package-lock.json" | "flake.lock" => Some("json"),
        "Gemfile.lock" => Some("ruby"),

        // Python
        "Pipfile" | "pyproject.toml" => Some("toml"),
        "setup.cfg" | "tox.ini" | ".flake8" | ".pylintrc" | ".pydocstyle"
        | "mypy.ini" | ".mypy.ini" | "pytest.ini" => Some("toml"),
        "requirements.txt" | "constraints.txt" | "MANIFEST.in" => Some("bash"),

        // Ruby
        "Gemfile" | "Rakefile" | "Vagrantfile" | "Guardfile"
        | "Berksfile" | "Thorfile" | "Capfile" | "Fastfile"
        | ".irbrc" | ".pryrc" | ".gemrc" | "config.ru" => Some("ruby"),

        // JavaScript / TypeScript configs
        ".babelrc" | ".eslintrc" | ".prettierrc" | ".stylelintrc"
        | ".swcrc" | ".nycrc" => Some("json"),
        "tsconfig.json" | "jsconfig.json" | "deno.json" | "deno.jsonc" => Some("json"),
        ".eslintrc.yml" | ".prettierrc.yml" | ".stylelintrc.yml" => Some("yaml"),

        // Go
        "go.sum" => Some("bash"),

        // Java / Kotlin / Gradle
        "build.gradle" | "settings.gradle" => Some("java"),
        "build.gradle.kts" | "settings.gradle.kts" => Some("kotlin"),
        "gradle.properties" | "local.properties" => Some("toml"),
        "pom.xml" | "ivy.xml" | "build.xml" => Some("html"),

        // Dart / Flutter
        "pubspec.yaml" | "analysis_options.yaml" => Some("yaml"),

        // Rust
        "rust-toolchain" | "rust-toolchain.toml" => Some("toml"),
        "clippy.toml" | "rustfmt.toml" | ".rustfmt.toml" => Some("toml"),

        // Git
        ".gitconfig" | ".gitattributes" | ".gitignore"
        | ".gitmodules" | ".mailmap" => Some("bash"),

        // Editor / IDE configs
        ".editorconfig" => Some("toml"),
        ".prettierignore" | ".eslintignore" | ".dockerignore"
        | ".npmignore" | ".slugignore" | ".cfignore"
        | ".helmignore" | ".vscodeignore" => Some("bash"),

        // CI / CD
        "Procfile" => Some("bash"),
        ".travis.yml" | ".gitlab-ci.yml" | "netlify.toml" | "vercel.json" => Some("yaml"),
        "cloudbuild.yaml" | "appveyor.yml" => Some("yaml"),

        // PHP / Laravel
        "artisan" => Some("php"),
        "composer.json" => Some("json"),

        // Misc configs
        ".npmrc" | ".yarnrc" | ".nvmrc" | ".node-version"
        | ".python-version" | ".ruby-version" | ".tool-versions" => Some("bash"),

        _ => None,
    };
    if let Some(lang) = by_filename {
        return Some(lang.to_string());
    }

    // Check for compound extensions (e.g., .blade.php)
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if file_stem.ends_with(".blade") {
        return Some("php".to_string());
    }
    if file_stem.ends_with(".test") || file_stem.ends_with(".spec") {
        // .test.js, .spec.ts etc. — fall through to normal ext detection
    }

    let ext = path.extension()?.to_str()?;
    let lang = match ext {
        // === Core languages ===
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "py" | "pyi" | "pyw" | "pyx" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" | "ipp" | "inl" | "tpp" => "cpp",
        "java" | "jav" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" | "sbt" => "scala",
        "swift" => "swift",
        "dart" => "dart",
        "lua" | "luau" => "lua",
        "rb" | "rake" | "gemspec" | "podspec" | "thor" | "irb" | "erb" => "ruby",
        "php" | "phtml" | "php3" | "php4" | "php5" | "phps" | "inc" => "php",

        // === Shell scripting ===
        "sh" | "bash" | "zsh" | "fish" | "ksh" | "csh" | "tcsh"
        | "bats" | "command" | "tool" => "bash",

        // === Data formats ===
        "json" | "jsonc" | "json5" | "geojson" | "webmanifest"
        | "har" | "jsonl" | "ndjson" | "ipynb" => "json",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "xml" | "xsl" | "xslt" | "xsd" | "dtd" | "wsdl" | "rss" | "atom"
        | "plist" | "csproj" | "fsproj" | "vbproj" | "vcxproj"
        | "sln" | "nuspec" | "resx" | "targets" | "props"
        | "androidmanifest" | "axml" | "iml" => "html",
        "svg" => "html",

        // === Web ===
        "html" | "htm" | "xhtml" | "ejs" | "hbs" | "handlebars"
        | "njk" | "nunjucks" | "liquid" | "mustache" | "jinja"
        | "jinja2" | "j2" | "tpl" => "html",
        "css" | "scss" | "less" | "sass" | "styl" | "stylus"
        | "postcss" | "pcss" => "css",

        // === Markdown & docs ===
        "md" | "markdown" | "mdx" | "rst" | "adoc" | "asciidoc"
        | "rmd" | "qmd" => "markdown",

        // === SQL ===
        "sql" | "mysql" | "pgsql" | "sqlite" | "plsql" | "tsql"
        | "cql" | "ddl" | "dml" => "sql",

        // === Config files (map to closest match) ===
        "ini" | "cfg" | "conf" | "cnf" | "inf" | "reg"
        | "properties" | "prop" | "env" | "flaskenv" => "toml",
        "lock" => "toml",

        // === Phase 2: proper grammars ===
        "cs" => "csharp",
        "zig" => "zig",
        "ex" | "exs" | "heex" | "leex" => "elixir",
        "r" | "rprofile" => "r",
        "svelte" => "svelte",
        "nix" => "nix",
        "hs" | "lhs" => "haskell",

        // === Build & CI ===
        "cmake" => "bash",
        "gradle" => "java",
        "tf" | "tfvars" | "hcl" => "toml",

        // === Misc recognized formats (fallback to closest grammar) ===
        "m" => "c",
        "mm" => "cpp",
        "pl" | "pm" | "pod" | "t" => "bash",
        "erl" | "hrl" => "bash",
        "fs" | "fsi" | "fsx" => "bash",
        "v" | "sv" | "svh" | "vh" => "c",
        "vhd" | "vhdl" => "bash",
        "nim" => "python",
        "cr" => "ruby",
        "proto" => "java",
        "graphql" | "gql" => "javascript",
        "wasm" | "wat" => "bash",

        _ => return None,
    };
    Some(lang.to_string())
}

/// Detect language from file content when extension-based detection fails.
/// Uses shebang lines, structural patterns, and keyword frequency analysis.
fn detect_language_from_content(content: &str) -> Option<String> {
    if content.trim().is_empty() {
        return None;
    }

    // Collect the first ~50 lines for analysis
    let lines: Vec<&str> = content.lines().take(50).collect();

    // Tier 1: Shebang line — highest confidence, instant match
    if let Some(first_line) = lines.first() {
        if let Some(lang) = detect_from_shebang(first_line) {
            return Some(lang.to_string());
        }
    }

    // Tier 2: Structural markers — single strong signals
    if let Some(lang) = detect_from_structure(&lines) {
        return Some(lang.to_string());
    }

    // Tier 3: Keyword frequency scoring across multiple languages
    detect_from_keywords(&lines)
}

fn detect_from_shebang(first_line: &str) -> Option<&'static str> {
    let line = first_line.trim();
    if !line.starts_with("#!") {
        return None;
    }
    let shebang = line.to_lowercase();

    if shebang.contains("python") {
        Some("python")
    } else if shebang.contains("node") || shebang.contains("deno") || shebang.contains("bun") {
        Some("javascript")
    } else if shebang.contains("ruby") {
        Some("ruby")
    } else if shebang.contains("perl") {
        Some("bash") // closest grammar
    } else if shebang.contains("bash") || shebang.contains("/sh") || shebang.contains("zsh") {
        Some("bash")
    } else if shebang.contains("php") {
        Some("php")
    } else if shebang.contains("lua") {
        Some("lua")
    } else {
        Some("bash") // generic shebang → shell-like
    }
}

fn detect_from_structure(lines: &[&str]) -> Option<&'static str> {
    let joined = lines.join("\n");
    let first = lines.first().map(|s| s.trim()).unwrap_or("");

    // JSON: starts with { or [ and contains "key": patterns
    if (first.starts_with('{') || first.starts_with('['))
        && (joined.contains("\":") || joined.contains("\": "))
    {
        return Some("json");
    }

    // XML/HTML: starts with <?xml or <!DOCTYPE or <html
    let first_lower = first.to_lowercase();
    if first_lower.starts_with("<?xml") {
        return Some("html"); // XML uses html grammar
    }
    if first_lower.starts_with("<!doctype") || first_lower.starts_with("<html") {
        return Some("html");
    }

    // YAML: starts with --- or has consistent key: value patterns
    if first == "---" {
        // Could be YAML frontmatter or YAML doc — check for key: value
        let kv_count = lines.iter().filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("---")
                && t.contains(": ") && !t.starts_with('"')
        }).count();
        if kv_count >= 2 {
            return Some("yaml");
        }
    }

    // TOML: [section] headers with key = value patterns
    let toml_section = lines.iter().any(|l| {
        let t = l.trim();
        t.starts_with('[') && t.ends_with(']') && !t.contains('"') && !t.contains(',')
    });
    let toml_kv = lines.iter().filter(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with('#') && !t.starts_with('[') && t.contains(" = ")
    }).count();
    if toml_section && toml_kv >= 2 {
        return Some("toml");
    }

    // SQL: starts with common SQL keywords
    let upper = first.to_uppercase();
    if upper.starts_with("SELECT ") || upper.starts_with("INSERT ")
        || upper.starts_with("CREATE ") || upper.starts_with("ALTER ")
        || upper.starts_with("DROP ") || upper.starts_with("WITH ")
        || upper.starts_with("-- ") && {
            // SQL comment followed by SQL keywords
            lines.iter().skip(1).take(5).any(|l| {
                let u = l.trim().to_uppercase();
                u.starts_with("SELECT") || u.starts_with("CREATE") || u.starts_with("INSERT")
            })
        }
    {
        return Some("sql");
    }

    None
}

fn detect_from_keywords(lines: &[&str]) -> Option<String> {
    let joined = lines.join("\n");

    // Score each language by counting distinctive patterns
    let mut scores: Vec<(&str, u32)> = Vec::new();

    // Python
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("def ") && t.contains(':') { s += 3; }
            if t.starts_with("class ") && t.contains(':') { s += 3; }
            if t.starts_with("import ") || t.starts_with("from ") && t.contains("import") { s += 3; }
            if t.starts_with("if __name__") { s += 5; }
            if t.starts_with("elif ") || t == "else:" { s += 2; }
            if t.starts_with("print(") { s += 3; }
            else if t.contains("print(") { s += 1; }
            if t.starts_with("@") && !t.contains('{') { s += 1; } // decorators
            if t.starts_with("# ") { s += 1; } // could be many langs though
        }
        if joined.contains("self.") { s += 2; }
        if joined.contains("None") || joined.contains("True") || joined.contains("False") { s += 1; }
        scores.push(("python", s));
    }

    // Rust
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("fn ") && t.contains("->") { s += 4; }
            if t.starts_with("fn ") { s += 2; }
            if t.starts_with("let mut ") || t.starts_with("let ") { s += 3; }
            if t.starts_with("use ") && t.contains("::") { s += 3; }
            if t.starts_with("pub fn ") || t.starts_with("pub struct ") || t.starts_with("pub enum ") { s += 4; }
            if t.starts_with("impl ") { s += 3; }
            if t.starts_with("mod ") { s += 2; }
            if t.starts_with("#[") || t.starts_with("#![") { s += 3; } // attributes
        }
        if joined.contains("unwrap()") || joined.contains(".expect(") { s += 2; }
        if joined.contains("Option<") || joined.contains("Result<") { s += 2; }
        scores.push(("rust", s));
    }

    // JavaScript
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("const ") || t.starts_with("let ") || t.starts_with("var ") { s += 2; }
            if t.contains("function ") || t.contains("function(") { s += 2; }
            if t.contains("=> {") || t.contains("=>") { s += 2; }
            if t.starts_with("import ") && t.contains("from ") { s += 3; }
            if t.starts_with("export ") { s += 3; }
            if t.contains("console.log") { s += 3; }
            if t.contains("require(") { s += 3; }
            if t.contains("document.") || t.contains("window.") { s += 2; }
        }
        if joined.contains("async ") || joined.contains("await ") { s += 1; }
        if joined.contains("null") || joined.contains("undefined") { s += 1; }
        scores.push(("javascript", s));
    }

    // TypeScript (extends JS with type annotations)
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.contains(": string") || t.contains(": number") || t.contains(": boolean") { s += 3; }
            if t.starts_with("interface ") || t.starts_with("type ") && t.contains('=') { s += 3; }
            if t.contains("as ") && (t.contains("string") || t.contains("any")) { s += 2; }
            if t.starts_with("import ") && t.contains("from ") { s += 2; }
            if t.starts_with("export ") { s += 2; }
            if t.contains("<") && t.contains(">") && t.contains(": ") { s += 1; } // generics + types
        }
        scores.push(("typescript", s));
    }

    // Go
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("package ") { s += 4; }
            if t.starts_with("func ") { s += 3; }
            if t == "import (" { s += 4; }
            if t.starts_with("import \"") { s += 3; }
            if t.contains(":= ") { s += 3; }
            if t.starts_with("type ") && (t.contains("struct") || t.contains("interface")) { s += 4; }
            if t.contains("fmt.") { s += 3; }
            if t.starts_with("if err != nil") { s += 5; }
        }
        if joined.contains("nil") { s += 1; }
        scores.push(("go", s));
    }

    // C
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("#include ") { s += 4; }
            if t.starts_with("#define ") || t.starts_with("#ifndef ") || t.starts_with("#ifdef ") { s += 3; }
            if t.contains("int main(") || t.contains("void main(") { s += 5; }
            if t.contains("printf(") || t.contains("fprintf(") { s += 3; }
            if t.contains("malloc(") || t.contains("free(") { s += 3; }
            if t.contains("NULL") { s += 1; }
            if t.contains("->") && t.contains(';') { s += 1; }
        }
        scores.push(("c", s));
    }

    // C++
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("#include <") && (t.contains("iostream") || t.contains("vector") || t.contains("string") || t.contains("memory")) { s += 5; }
            if t.starts_with("#include ") { s += 2; }
            if t.contains("std::") { s += 4; }
            if t.contains("cout") || t.contains("cin") || t.contains("endl") { s += 3; }
            if t.starts_with("class ") && t.contains('{') { s += 2; }
            if t.starts_with("namespace ") { s += 3; }
            if t.contains("template<") || t.contains("template <") { s += 4; }
            if t.contains("nullptr") { s += 3; }
        }
        scores.push(("cpp", s));
    }

    // Java
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("package ") && t.contains(';') { s += 4; }
            if t.starts_with("import ") && t.contains(';') && t.contains('.') { s += 3; }
            if t.contains("public class ") || t.contains("public interface ") { s += 5; }
            if t.contains("public static void main") { s += 5; }
            if t.contains("System.out.print") { s += 4; }
            if t.starts_with("@Override") || t.starts_with("@Autowired") { s += 3; }
            if t.contains("private ") || t.contains("protected ") { s += 1; }
        }
        scores.push(("java", s));
    }

    // PHP
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("<?php") { s += 10; }
            if t.starts_with("<?") && !t.starts_with("<?xml") { s += 5; }
            if t.contains("$") && t.contains(';') { s += 2; } // PHP variables
            if t.contains("echo ") || t.contains("var_dump(") { s += 3; }
            if t.starts_with("namespace ") && t.contains('\\') { s += 4; }
        }
        scores.push(("php", s));
    }

    // Ruby
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("require ") && t.contains("'") { s += 3; }
            if t.starts_with("def ") && !t.contains(':') { s += 3; } // ruby def without colon (vs python)
            if t == "end" { s += 2; }
            if t.starts_with("class ") && !t.contains('{') { s += 2; }
            if t.starts_with("module ") { s += 3; }
            if t.contains(".each ") || t.contains(".map ") || t.contains(".select ") { s += 2; }
            if t.contains(" do |") || t.contains(" do\n") { s += 3; }
            if t.starts_with("puts ") { s += 3; }
            if t.contains("attr_accessor") || t.contains("attr_reader") { s += 4; }
        }
        scores.push(("ruby", s));
    }

    // Bash/Shell
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("if [") || t.starts_with("if [[") { s += 3; }
            if t == "fi" || t == "done" || t == "esac" { s += 3; }
            if t.starts_with("echo ") { s += 2; }
            if t.starts_with("export ") { s += 2; }
            if t.contains("$(" ) || t.contains("${") { s += 2; }
            if t.starts_with("for ") && t.contains(" in ") { s += 2; }
            if t.starts_with("while ") || t.starts_with("case ") { s += 2; }
            if t.starts_with("function ") && !t.contains('{') && !t.contains('(') { s += 2; }
        }
        scores.push(("bash", s));
    }

    // CSS
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.ends_with('{') && (t.starts_with('.') || t.starts_with('#') || t.starts_with("@media")) { s += 3; }
            if t.contains("color:") || t.contains("margin:") || t.contains("padding:")
                || t.contains("display:") || t.contains("font-size:") { s += 3; }
            if t.starts_with("@import ") || t.starts_with("@keyframes ") { s += 3; }
        }
        scores.push(("css", s));
    }

    // Markdown
    {
        let mut s: u32 = 0;
        for line in lines {
            let t = line.trim();
            if t.starts_with("# ") || t.starts_with("## ") || t.starts_with("### ") { s += 2; }
            if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("1. ") { s += 1; }
            if t.starts_with("```") { s += 3; }
            if t.contains("](") && t.contains('[') { s += 2; } // links
            if t.starts_with("> ") { s += 1; }
        }
        scores.push(("markdown", s));
    }

    // Scale threshold by file size — short files have fewer lines to score from
    let threshold: u32 = if lines.len() <= 3 { 2 } else if lines.len() <= 10 { 3 } else { 5 };

    // Find the highest scoring language, with a gap over the runner-up for confidence
    scores.sort_by(|a, b| b.1.cmp(&a.1));
    if let Some((lang, score)) = scores.first() {
        if *score >= threshold {
            return Some(lang.to_string());
        }
    }
    None
}
