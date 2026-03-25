use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_path: PathBuf,
    pub is_expanded: bool,
}

impl Project {
    pub fn new(root_path: PathBuf) -> Self {
        let name = root_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root_path.to_string_lossy().to_string());

        Self {
            id: Uuid::new_v4().to_string(),
            name,
            root_path,
            is_expanded: true,
        }
    }
}
