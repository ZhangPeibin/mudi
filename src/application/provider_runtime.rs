//! Provider runtime contracts consumed by authorized application workflows.

use crate::domain::Provider;
use std::path::{Path, PathBuf};

pub trait ProviderExecutableResolver: Send + Sync {
    fn resolve_executable(&self, provider: Provider) -> Option<PathBuf>;
}

/// Configured Provider CLI executables. Discovery and validation belong to adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExecutables {
    claude: PathBuf,
    codex: PathBuf,
}

impl ProviderExecutables {
    pub fn new(claude: PathBuf, codex: PathBuf) -> Self {
        Self { claude, codex }
    }

    pub fn executable_for(&self, provider: Provider) -> Option<&Path> {
        let path = match provider {
            Provider::ClaudeCode => &self.claude,
            Provider::CodexCli => &self.codex,
        };
        (!path.as_os_str().is_empty()).then_some(path.as_path())
    }

    pub fn claude_path(&self) -> &Path {
        &self.claude
    }

    pub fn codex_path(&self) -> &Path {
        &self.codex
    }
}
