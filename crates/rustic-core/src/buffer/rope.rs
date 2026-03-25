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
    let ext = path.extension()?.to_str()?;
    let lang = match ext {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => "cpp",
        "java" => "java",
        "json" => "json",
        "toml" => "toml",
        "html" | "htm" => "html",
        "css" => "css",
        "md" | "markdown" => "markdown",
        "svg" | "xml" => "html",
        _ => return None,
    };
    Some(lang.to_string())
}
