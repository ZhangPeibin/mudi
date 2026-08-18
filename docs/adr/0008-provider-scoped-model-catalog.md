# ADR 0008 — Use a Model-Provider-scoped Model Catalog

- **Status:** Accepted and implemented (2026-08-03).
- **Supersedes:** ADR 0001 only where it assigned inferred Context Windows to
  `ProviderAdapter`.

## Context

API prices lived in `pricing/models.json`, keyed only by model name, while inferred Claude
Context Windows lived in adapter code. This split made one exact model carry unrelated
configuration in two places, confused Session Providers with model vendors, and had no
representation for Batch prices or source provenance.

Codex records `model_context_window` in its source log. Claude does not, so the two Providers
also need different precedence between observed and configured window data.

## Decision

Use one bundled `models/catalog.json`, loaded by `models::ModelCatalog`, with this hierarchy:

```text
Model Provider
└── exact model identifier
    ├── input / output / cached / cache_write (USD per million tokens)
    ├── batch { input / output } (optional metadata)
    └── context_window (tokens, optional)
```

The Model Provider is the API vendor (`anthropic`, `openai`, `google`, and so on), not the
Session Provider (`claude` or `codex`). Direct catalog access requires both the exact Model
Provider and model identifier. A runtime Session that only exposes a model identifier may
resolve it when exactly one Model Provider owns that identifier; unknown and ambiguous IDs
remain `N/A` and never use a nearby model.

The bundled public snapshot imports the structured pricing table from
`llmcalcs.com/pricing-comparison`, records its source update and retrieval dates, and retains
the per-model official pricing URL exposed by that table. Exact runtime IDs already observed
by Mudi but absent from the imported source remain explicit supplemental entries.

Claude's exact Catalog entry owns its Context Window because Claude JSONL has no native
window field. Codex's provider-native window wins, with the Catalog only as fallback. This
precedence also replaces stale inferred Claude windows already present in the disposable
index.

Interactive Session estimates use direct prices only. Missing Cached or Batch prices remain
absent rather than becoming zero; Batch prices are descriptive and do not participate in
current cost calculation. A complete local `models.json` can override individual exact
entries under `MUDI_DATA_DIR`.

## Consequences

- `pricing/models.json`, `src/pricing.rs`, `RateCard`, and `pricing.json` are replaced by
  `models/catalog.json`, `src/adapters/models.rs`, `ModelCatalog`, and `models.json`.
- Context Window inference leaves `ProviderAdapter`; provider-native parsing remains there.
- Session Provider and Model Provider remain distinct domain concepts.
- Adding prices or a Context Window is a data change rather than an adapter code change.
