//! Session Provider and Provider source metadata.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    ClaudeCode,
    CodexCli,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceFormat {
    #[default]
    JsonLines,
}

impl SourceFormat {
    pub fn label(self) -> &'static str {
        match self {
            SourceFormat::JsonLines => "JSONL",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub path: PathBuf,
    pub format: SourceFormat,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "claude",
            Provider::CodexCli => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code",
            Provider::CodexCli => "Codex CLI",
        }
    }

    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Provider::ClaudeCode),
            "codex" => Some(Provider::CodexCli),
            _ => None,
        }
    }
}
