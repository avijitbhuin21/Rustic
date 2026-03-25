pub mod file_tree;
pub mod project;

use project::Project;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct Workspace {
    pub projects: Vec<Project>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            projects: Vec::new(),
        }
    }

    pub fn add_project(&mut self, path: PathBuf) -> Project {
        // Check if already added
        if let Some(existing) = self.projects.iter().find(|p| p.root_path == path) {
            return existing.clone();
        }

        let project = Project::new(path);
        self.projects.push(project.clone());
        project
    }

    pub fn remove_project(&mut self, id: &str) {
        self.projects.retain(|p| p.id != id);
    }

    pub fn list_projects(&self) -> Vec<Project> {
        self.projects.clone()
    }
}
