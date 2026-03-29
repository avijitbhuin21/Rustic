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
        let language = detect_language(path);

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
