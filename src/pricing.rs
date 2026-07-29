//! Pricing: a bundled, versioned rate card with optional local overrides.
//!
//! Produces API-equivalent dollar estimates. Never silently substitutes a nearby
//! model — an unknown model yields `None`, which the UI renders as `N/A`.

use crate::domain::Usage;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Bundled rate card, embedded at build time.
const BUNDLED: &str = include_str!("../pricing/models.json");

/// Per-million-token rates for a single model.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct RateFile {
    version: String,
    #[serde(default)]
    models: HashMap<String, ModelRate>,
}

/// Resolved pricing table.
#[derive(Debug, Clone)]
pub struct RateCard {
    pub version: String,
    models: HashMap<String, ModelRate>,
}

impl RateCard {
    /// Load the bundled card, then merge any local override file on top.
    pub fn load(override_path: Option<&Path>) -> Self {
        let base: RateFile = serde_json::from_str(BUNDLED)
            .expect("bundled pricing/models.json must be valid");
        let mut models = base.models;
        let mut version = base.version;

        if let Some(path) = override_path {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(over) = serde_json::from_str::<RateFile>(&text) {
                    version = format!("{}+override", version);
                    for (k, v) in over.models {
                        models.insert(k, v);
                    }
                }
            }
        }

        RateCard { version, models }
    }

    pub fn rate(&self, model: &str) -> Option<&ModelRate> {
        self.models.get(model)
    }

    /// Estimate dollars for `usage` under `model`. `None` if the model is unknown.
    pub fn estimate(&self, model: &str, usage: &Usage) -> Option<f64> {
        let r = self.models.get(model)?;
        let per_m = 1_000_000.0;
        let cost = usage.input as f64 * r.input / per_m
            + usage.output as f64 * r.output / per_m
            + usage.cache_read as f64 * r.cache_read / per_m
            + usage.cache_write as f64 * r.cache_write / per_m;
        Some(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_card_parses() {
        let card = RateCard::load(None);
        assert!(!card.version.is_empty());
        assert!(card.rate("claude-opus-4-8").is_some());
    }

    #[test]
    fn unknown_model_is_none_never_substituted() {
        let card = RateCard::load(None);
        let usage = Usage { input: 1_000_000, output: 0, cache_read: 0, cache_write: 0 };
        assert_eq!(card.estimate("totally-unknown-model", &usage), None);
    }

    #[test]
    fn estimate_uses_per_category_rates() {
        let card = RateCard::load(None);
        // Opus: input 15/M, output 75/M.
        let usage = Usage { input: 1_000_000, output: 1_000_000, cache_read: 0, cache_write: 0 };
        let cost = card.estimate("claude-opus-4-8", &usage).unwrap();
        assert!((cost - 90.0).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn cache_categories_counted_separately() {
        let card = RateCard::load(None);
        let usage = Usage { input: 0, output: 0, cache_read: 1_000_000, cache_write: 1_000_000 };
        // Opus: cache_read 1.5/M, cache_write 18.75/M.
        let cost = card.estimate("claude-opus-4-8", &usage).unwrap();
        assert!((cost - 20.25).abs() < 1e-9, "got {cost}");
    }
}
