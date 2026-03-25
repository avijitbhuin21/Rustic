use anyhow::Result;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct SearchQuery {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub paths: Vec<PathBuf>,
    pub include_glob: Option<String>,
    pub exclude_glob: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchMatch {
    pub line_number: usize,
    pub line_text: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub file_path: String,
    pub matches: Vec<SearchMatch>,
}

pub struct SearchEngine;

impl SearchEngine {
    /// Replace all occurrences of a pattern in a single file.
    /// Returns the number of replacements made.
    pub fn replace_in_file(
        file_path: &str,
        pattern: &str,
        replacement: &str,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    ) -> Result<u32> {
        let query = SearchQuery {
            pattern: pattern.to_string(),
            is_regex,
            case_sensitive,
            whole_word,
            paths: vec![],
            include_glob: None,
            exclude_glob: None,
        };
        let regex = Self::build_regex(&query)?;
        let content = fs::read_to_string(file_path)?;
        let uses_crlf = content.contains("\r\n");
        let line_ending = if uses_crlf { "\r\n" } else { "\n" };
        let mut count = 0u32;
        let new_content: String = content
            .lines()
            .map(|line| {
                let matches_in_line = regex.find_iter(line).count() as u32;
                count += matches_in_line;
                regex.replace_all(line, replacement).into_owned()
            })
            .collect::<Vec<_>>()
            .join(line_ending);

        // Preserve trailing newline if original had one
        let final_content = if content.ends_with('\n') || content.ends_with("\r\n") {
            format!("{}{}", new_content, line_ending)
        } else {
            new_content
        };

        if count > 0 {
            fs::write(file_path, final_content)?;
        }
        Ok(count)
    }

    /// Search for a pattern across all given paths.
    /// Returns results grouped by file.
    pub fn search(query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let regex = Self::build_regex(query)?;
        let mut results = Vec::new();

        for search_path in &query.paths {
            let mut walker = WalkBuilder::new(search_path);
            walker
                .hidden(true)       // skip hidden files
                .git_ignore(true)   // respect .gitignore
                .max_depth(None);

            for entry in walker.build().flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                // Apply include/exclude glob filters
                if let Some(ref include) = query.include_glob {
                    if let Ok(glob) = glob::Pattern::new(include) {
                        if !glob.matches_path(path) {
                            continue;
                        }
                    }
                }
                if let Some(ref exclude) = query.exclude_glob {
                    if let Ok(glob) = glob::Pattern::new(exclude) {
                        if glob.matches_path(path) {
                            continue;
                        }
                    }
                }

                // Read file — skip binary/large files
                let content = match fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue, // skip binary or unreadable files
                };

                let mut matches = Vec::new();
                for (i, line) in content.lines().enumerate() {
                    for mat in regex.find_iter(line) {
                        matches.push(SearchMatch {
                            line_number: i + 1,
                            line_text: line.to_string(),
                            match_start: mat.start(),
                            match_end: mat.end(),
                        });
                    }
                }

                if !matches.is_empty() {
                    results.push(SearchResult {
                        file_path: path.to_string_lossy().to_string(),
                        matches,
                    });
                }
            }
        }

        Ok(results)
    }

    fn build_regex(query: &SearchQuery) -> Result<Regex> {
        let mut pattern = if query.is_regex {
            query.pattern.clone()
        } else {
            regex::escape(&query.pattern)
        };

        if query.whole_word {
            pattern = format!(r"\b{}\b", pattern);
        }

        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!query.case_sensitive)
            .build()?;

        Ok(regex)
    }
}
