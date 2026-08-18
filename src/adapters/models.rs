//! Model-Provider-scoped catalog: prices, batch prices, and context windows.
//!
//! Interactive Session estimates use the direct token prices. Batch prices are
//! descriptive model metadata and are never substituted into observed CLI usage.

use crate::domain::{Provider, Usage};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Bundled model catalog, embedded at build time.
const BUNDLED: &str = include_str!("../../models/catalog.json");

/// Per-million-token batch rates, when the Model Provider supports them.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BatchPrices {
    pub input: f64,
    pub output: f64,
}

/// Model-Provider-specific metadata for one exact model identifier.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModelDefinition {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cached: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub batch: Option<BatchPrices>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub pricing_source: Option<String>,
}

/// Provenance for the imported public model data.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CatalogSource {
    pub url: String,
    pub data_last_updated: String,
    pub retrieved_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogFile {
    version: String,
    currency: String,
    unit: String,
    #[serde(default)]
    source: Option<CatalogSource>,
    #[serde(default, rename = "providers")]
    model_providers: HashMap<String, HashMap<String, ModelDefinition>>,
}

/// Resolved Model-Provider-scoped catalog.
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    pub version: String,
    pub currency: String,
    pub unit: String,
    pub source: Option<CatalogSource>,
    model_providers: HashMap<String, HashMap<String, ModelDefinition>>,
}

impl ModelCatalog {
    /// Load the bundled catalog, then merge an optional local catalog on top.
    pub fn load(override_path: Option<&Path>) -> Self {
        let base: CatalogFile =
            serde_json::from_str(BUNDLED).expect("bundled models/catalog.json must be valid");
        let mut model_providers = base.model_providers;
        let mut version = base.version;
        let mut currency = base.currency;
        let mut unit = base.unit;
        let mut source = base.source;

        if let Some(path) = override_path {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(over) = serde_json::from_str::<CatalogFile>(&text) {
                    version = format!("{}+override", version);
                    currency = over.currency;
                    unit = over.unit;
                    source = over.source.or(source);
                    for (provider, models) in over.model_providers {
                        model_providers.entry(provider).or_default().extend(models);
                    }
                }
            }
        }

        ModelCatalog {
            version,
            currency,
            unit,
            source,
            model_providers,
        }
    }

    pub fn model(&self, model_provider: &str, model: &str) -> Option<&ModelDefinition> {
        self.model_providers.get(model_provider)?.get(model)
    }

    /// Resolve an exact model identifier only when it belongs to one Model Provider.
    /// Ambiguous identifiers remain unknown until a Session source can identify the
    /// Model Provider explicitly.
    pub fn model_by_id(&self, model: &str) -> Option<&ModelDefinition> {
        let mut matches = self
            .model_providers
            .values()
            .filter_map(|models| models.get(model));
        let definition = matches.next()?;
        matches.next().is_none().then_some(definition)
    }

    pub fn context_window(&self, model: &str) -> Option<u64> {
        self.model_by_id(model)?.context_window
    }

    /// Resolve the current window using the most authoritative source available.
    /// Claude JSONL has no window, so the catalog owns it. Codex records the
    /// window per turn, so provider-native evidence wins there.
    pub fn resolve_context_window(
        &self,
        session_provider: Provider,
        model: &str,
        provider_native: Option<u64>,
    ) -> Option<u64> {
        match session_provider {
            Provider::ClaudeCode => self.context_window(model),
            Provider::CodexCli => provider_native.or_else(|| self.context_window(model)),
        }
    }

    /// Estimate interactive API-equivalent dollars for observed Session usage.
    pub fn estimate(&self, model: &str, usage: &Usage) -> Option<f64> {
        let definition = self.model_by_id(model)?;
        let per_million = 1_000_000.0;
        let cached = match usage.cache_read {
            0 => 0.0,
            tokens => tokens as f64 * definition.cached?,
        };
        // Most APIs bill cache creation as ordinary input. Anthropic's distinct
        // cache-write rate is recorded explicitly and overrides this fallback.
        let cache_write_rate = definition.cache_write.unwrap_or(definition.input);
        Some(
            (usage.input as f64 * definition.input
                + usage.output as f64 * definition.output
                + cached
                + usage.cache_write as f64 * cache_write_rate)
                / per_million,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bundled_catalog_is_scoped_by_model_provider_and_includes_source_metadata() {
        let catalog = ModelCatalog::load(None);
        let opus = catalog.model("anthropic", "claude-opus-4.8").unwrap();

        assert_eq!(opus.context_window, Some(1_000_000));
        assert_eq!(catalog.currency, "USD");
        assert_eq!(catalog.unit, "per_million_tokens");
        assert_eq!(opus.input, 5.0);
        assert_eq!(opus.output, 25.0);
        assert_eq!(opus.cached, Some(0.5));
        assert!(opus.batch.is_some());
        assert_eq!(catalog.model_providers.len(), 9);
        assert_eq!(
            catalog
                .model_providers
                .values()
                .map(HashMap::len)
                .sum::<usize>(),
            80
        );
        assert_eq!(
            catalog.source.as_ref().unwrap().data_last_updated,
            "2026-06-11"
        );
        assert!(catalog.model("openai", "claude-opus-4.8").is_none());
    }

    #[test]
    fn unknown_model_is_none_never_substituted() {
        let catalog = ModelCatalog::load(None);
        let usage = Usage {
            input: 1_000_000,
            ..Usage::default()
        };

        assert_eq!(catalog.estimate("totally-unknown-model", &usage), None);
    }

    #[test]
    fn estimate_uses_interactive_prices_not_batch_prices() {
        let catalog = ModelCatalog::load(None);
        let usage = Usage {
            input: 1_000_000,
            output: 1_000_000,
            ..Usage::default()
        };

        let cost = catalog.estimate("claude-opus-4.8", &usage).unwrap();

        assert!((cost - 30.0).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn cache_read_and_cache_write_prices_remain_separate() {
        let catalog = ModelCatalog::load(None);
        let usage = Usage {
            cache_read: 1_000_000,
            cache_write: 1_000_000,
            ..Usage::default()
        };

        let cost = catalog.estimate("claude-opus-4.8", &usage).unwrap();

        assert!((cost - 6.75).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn used_cache_category_requires_a_known_price() {
        let catalog = ModelCatalog::load(None);
        let usage = Usage {
            cache_read: 1_000_000,
            ..Usage::default()
        };

        assert_eq!(catalog.estimate("gpt-5.5-pro", &usage), None);
    }

    #[test]
    fn context_window_uses_the_unique_exact_model_entry() {
        let catalog = ModelCatalog::load(None);

        assert_eq!(catalog.context_window("claude-opus-4.8"), Some(1_000_000));
        assert_eq!(
            catalog.context_window("claude-haiku-4-5-20251001"),
            Some(200_000)
        );
        assert_eq!(catalog.context_window("gpt-5.6-sol"), Some(1_050_000));
    }

    #[test]
    fn context_window_resolution_respects_provider_native_authority() {
        let catalog = ModelCatalog::load(None);

        assert_eq!(
            catalog.resolve_context_window(Provider::ClaudeCode, "claude-opus-4.8", Some(200_000)),
            Some(1_000_000),
            "the catalog replaces stale inferred Claude windows"
        );
        assert_eq!(
            catalog.resolve_context_window(Provider::CodexCli, "gpt-5.6-sol", Some(258_400)),
            Some(258_400),
            "Codex provider-native windows remain authoritative"
        );
    }

    #[test]
    fn duplicate_model_ids_across_model_providers_are_not_guessed() {
        let mut catalog = ModelCatalog::load(None);
        let opus = catalog
            .model("anthropic", "claude-opus-4.8")
            .unwrap()
            .clone();
        catalog
            .model_providers
            .get_mut("openai")
            .unwrap()
            .insert("claude-opus-4.8".to_string(), opus);

        assert_eq!(catalog.model_by_id("claude-opus-4.8"), None);
    }

    #[test]
    fn local_catalog_merges_exact_provider_model_entries() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mudi-model-catalog-{}-{nonce}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
                "version": "local",
                "currency": "USD",
                "unit": "per_million_tokens",
                "providers": {
                    "anthropic": {
                        "claude-opus-4.8": {
                            "input": 99.0,
                            "output": 98.0,
                            "cached": 97.0,
                            "cache_write": 96.0,
                            "batch": null,
                            "context_window": 123456
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let catalog = ModelCatalog::load(Some(&path));
        std::fs::remove_file(path).unwrap();

        let overridden = catalog.model("anthropic", "claude-opus-4.8").unwrap();
        assert_eq!(catalog.version, "2026-08-03+override");
        assert_eq!(overridden.input, 99.0);
        assert_eq!(overridden.context_window, Some(123_456));
        assert!(catalog.model("anthropic", "claude-haiku-4.5").is_some());
        assert!(catalog.model("openai", "gpt-5.6-sol").is_some());
    }
}
