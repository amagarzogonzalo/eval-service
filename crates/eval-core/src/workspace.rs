use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// A workspace file registering projects for portfolio views.
/// Relative paths resolve against the directory containing the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceFile {
    #[serde(default)]
    pub workspace: WorkspaceMeta,
    #[serde(default)]
    pub project: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    #[serde(default)]
    pub name: String,
}

/// One registered project. Nothing is ever written into the project;
/// the platform only reads its suites and run store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    /// Stable id. Namespaces all suites and runs of this project.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Local repository location (relative to the workspace file, or absolute).
    #[serde(default = "default_repo")]
    pub repo: String,
    /// Suite files relative to the repo dir. May be empty.
    #[serde(default)]
    pub suites: Vec<String>,
    /// Run store dir relative to the repo dir.
    #[serde(default = "default_store")]
    pub store: String,
}

fn default_repo() -> String {
    ".".to_string()
}

fn default_store() -> String {
    "eval-runs".to_string()
}

impl WorkspaceFile {
    /// Load and validate a workspace file.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CoreError::io(path.display().to_string(), e))?;
        let file: Self =
            toml::from_str(&text).map_err(|e| CoreError::Config(format!("workspace: {e}")))?;
        file.validate()?;
        Ok(file)
    }

    fn validate(&self) -> Result<(), CoreError> {
        let mut seen = HashSet::new();
        for project in &self.project {
            if project.id.trim().is_empty() {
                return Err(CoreError::Config(
                    "project id must be non-empty".to_string(),
                ));
            }
            if !seen.insert(project.id.as_str()) {
                return Err(CoreError::Config(format!(
                    "duplicate project id `{}`",
                    project.id
                )));
            }
            if project.name.trim().is_empty() {
                return Err(CoreError::Config(format!(
                    "project `{}`: name must be non-empty",
                    project.id
                )));
            }
        }
        Ok(())
    }

    /// Directory the workspace file lives in (anchor for relative paths).
    pub fn anchor(path: &Path) -> std::path::PathBuf {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }
}

impl ProjectEntry {
    /// Resolve the repo dir against the workspace anchor.
    pub fn repo_dir(&self, anchor: &Path) -> std::path::PathBuf {
        let repo = Path::new(&self.repo);
        if repo.is_absolute() {
            repo.to_path_buf()
        } else {
            anchor.join(repo)
        }
    }

    /// Resolve the store dir against the repo dir.
    pub fn store_dir(&self, anchor: &Path) -> std::path::PathBuf {
        let store = Path::new(&self.store);
        if store.is_absolute() {
            store.to_path_buf()
        } else {
            self.repo_dir(anchor).join(store)
        }
    }

    /// Resolve a suite path against the repo dir.
    pub fn suite_path(&self, anchor: &Path, suite: &str) -> std::path::PathBuf {
        let path = Path::new(suite);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.repo_dir(anchor).join(path)
        }
    }
}
