use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub byte_offset: usize,
    pub old_text: String,
    pub new_text: String,
}

impl Edit {
    pub fn inverse(&self) -> Edit {
        Edit {
            byte_offset: self.byte_offset,
            old_text: self.new_text.clone(),
            new_text: self.old_text.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EditGroup {
    pub edits: Vec<Edit>,
}
