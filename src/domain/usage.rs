//! Context occupancy, processed token usage, and Compaction metadata.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextState {
    pub tokens: u64,
    pub window: Option<u64>,
}

impl ContextState {
    pub fn percent(&self) -> Option<f64> {
        match self.window {
            Some(w) if w > 0 => Some((self.tokens as f64 / w as f64) * 100.0),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub usage: Usage,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub usage: Usage,
    pub per_model: Vec<ModelUsage>,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Compaction {
    pub at_ms: i64,
    pub trigger: Option<String>,
    pub pre_tokens: Option<u64>,
}
