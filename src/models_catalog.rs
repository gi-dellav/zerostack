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
    #[serde(default)]
    input_price: Option<f64>,
    #[serde(default)]
    output_price: Option<f64>,
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
                    input_price: m.input_price,
                    output_price: m.output_price,
                })
                .collect();
            (provider, entries)
        })
        .collect()
});

/// Baked model entries for a provider, or `None` when the provider is not in the
/// catalog (custom gateways, ollama — those resolve live).
pub fn catalog_entries(provider: &str) -> Option<&'static [ModelEntry]> {
    CATALOG.get(provider).map(|v| v.as_slice())
}
