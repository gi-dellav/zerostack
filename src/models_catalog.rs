//! Static, embedded model catalog.
//!
//! Model ids change rarely between releases, so instead of hitting each
//! provider's `/models` endpoint at startup (slow — OpenRouter alone returns
//! hundreds of entries and used to block the first frame), we bake a snapshot
//! into the binary. The picker is seeded from this synchronously, with zero
//! network. The live listing is still available on demand via `/models refresh`
//! (see [`crate::ui::slash`]) and for providers not baked here (custom gateways,
//! ollama).
//!
//! The data lives in `data/models.json`, keyed by *zerostack* provider name
//! (so `gemini`, not models.dev's `google`). Refresh it with
//! `scripts/gen-models-catalog.sh`.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::provider::ModelEntry;

const CATALOG_JSON: &str = include_str!("../data/models.json");

#[derive(serde::Deserialize)]
struct RawModel {
    id: String,
    name: String,
    context: Option<u32>,
}

static CATALOG: LazyLock<HashMap<String, Vec<ModelEntry>>> = LazyLock::new(|| {
    let raw: HashMap<String, Vec<RawModel>> = serde_json::from_str(CATALOG_JSON)
        .expect("embedded data/models.json is malformed — run scripts/gen-models-catalog.sh");
    raw.into_iter()
        .map(|(provider, models)| {
            let entries = models
                .into_iter()
                .map(|m| ModelEntry {
                    id: m.id,
                    display: m.name,
                    context_length: m.context,
                    kind: None,
                })
                .collect();
            (provider, entries)
        })
        .collect()
});

/// Baked model entries for a provider, or `None` when the provider is not in the
/// catalog (custom gateways, ollama — those resolve live).
pub fn catalog_entries(provider: &str) -> Option<&'static [ModelEntry]> {
    let provider = match provider {
        "openai-codex" | "codex" => "openai",
        other => other,
    };
    CATALOG.get(provider).map(|v| v.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(provider: &str) -> Vec<String> {
        catalog_entries(provider)
            .unwrap_or(&[])
            .iter()
            .map(|m| m.id.clone())
            .collect()
    }

    #[test]
    fn catalog_parses_and_has_expected_providers() {
        for p in ["anthropic", "deepseek", "openai", "gemini", "openrouter"] {
            assert!(
                !ids(p).is_empty(),
                "missing or empty baked catalog for: {p}"
            );
        }
    }

    #[test]
    fn deepseek_includes_default_models() {
        // The direct DeepSeek defaults must be discoverable offline so the picker
        // is useful on a fresh, network-blocked start.
        let ids = ids("deepseek");
        assert!(ids.contains(&"deepseek-v4-flash".to_string()));
        assert!(ids.contains(&"deepseek-v4-pro".to_string()));
    }

    #[test]
    fn openrouter_includes_deepseek_provider_models() {
        // The OpenRouter-prefixed DeepSeek models remain available when using
        // OpenRouter explicitly.
        // offline so the picker is useful on a fresh, network-blocked start.
        assert!(
            ids("openrouter").contains(&"deepseek/deepseek-v4-pro".to_string()),
            "default model missing from baked openrouter catalog"
        );
    }

    #[test]
    fn unbaked_provider_has_no_catalog() {
        // ollama resolves live (local), so it is intentionally not baked.
        assert!(catalog_entries("ollama").is_none());
    }

    #[test]
    fn openai_codex_uses_openai_catalog() {
        let openai = catalog_entries("openai").unwrap();
        let codex = catalog_entries("openai-codex").unwrap();
        assert_eq!(openai.len(), codex.len());
        assert!(codex.iter().any(|m| m.id == "gpt-5.5"));
    }
}
