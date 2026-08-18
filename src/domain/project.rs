//! Canonical Project-directory identity.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectIdentity(PathBuf);

impl ProjectIdentity {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn display_path(&self) -> &std::path::Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub identity: ProjectIdentity,
    pub label: String,
}
