use crate::models_catalog::catalog_entries;

fn ids(provider: &str) -> Vec<String> {
    catalog_entries(provider)
        .unwrap_or(&[])
        .iter()
        .map(|m| m.id.clone())
        .collect()
}

#[test]
fn catalog_parses_and_has_expected_providers() {
    for p in [
        "anthropic",
        "openai",
        "gemini",
        "openrouter",
        "opencode-zen",
        "opencode-go",
    ] {
        assert!(
            !ids(p).is_empty(),
            "missing or empty baked catalog for: {p}"
        );
    }
}

#[test]
fn openrouter_includes_default_model() {
    // The default model (deepseek-v4-pro on openrouter) must be discoverable
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
