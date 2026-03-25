use ignore::WalkBuilder;
use std::path::PathBuf;

/// Quick file name search — finds files whose name contains the query string.
/// Used for Ctrl+P file picker.
pub fn find_files(query: &str, paths: &[PathBuf], max_results: usize) -> Vec<PathBuf> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for search_path in paths {
        let walker = WalkBuilder::new(search_path)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.to_lowercase().contains(&query_lower) {
                    results.push(path.to_path_buf());
                    if results.len() >= max_results {
                        return results;
                    }
                }
            }
        }
    }

    results
}
