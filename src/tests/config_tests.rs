use crate::config::Config;
use crate::config::types::CustomProviderConfig;
use compact_str::CompactString;
use std::collections::HashMap;

fn custom_provider(provider_type: &str) -> CustomProviderConfig {
    CustomProviderConfig {
        provider_type: CompactString::new(provider_type),
        base_url: "https://gateway.example.com".to_string(),
        api_key_env: None,
        danger_accept_invalid_certs: None,
        api_style: None,
        headers: HashMap::new(),
        timeout_secs: None,
        model: None,
    }
}

#[test]
fn is_anthropic_native_builtin_providers() {
    let cfg = Config::default();
    assert!(cfg.is_anthropic_native("anthropic"));
    assert!(cfg.is_anthropic_native("Anthropic")); // case-insensitive
    for p in ["openai", "gemini", "google", "openrouter", "ollama"] {
        assert!(!cfg.is_anthropic_native(p), "{p} is not anthropic-native");
    }
}

#[test]
fn is_anthropic_native_resolves_custom_provider_type() {
    // A custom gateway named anything but routing through the Anthropic-native
    // protocol must be treated as anthropic-native (so cache fields are added),
    // while an OpenAI-style gateway must not.
    let mut providers = HashMap::new();
    providers.insert("my-claude-proxy".to_string(), custom_provider("anthropic"));
    providers.insert("my-oai-gateway".to_string(), custom_provider("openai"));
    let cfg = Config {
        custom_providers: Some(providers),
        ..Config::default()
    };
    assert!(cfg.is_anthropic_native("my-claude-proxy"));
    assert!(!cfg.is_anthropic_native("my-oai-gateway"));
    // Unknown name with no custom entry falls back to the literal kind.
    assert!(!cfg.is_anthropic_native("totally-unknown"));
}

#[test]
fn mid_turn_threshold_unset_by_default() {
    let cfg = Config::default();
    assert_eq!(cfg.resolve_mid_turn_compact_threshold(), None);
}

#[test]
fn mid_turn_threshold_valid_value_passes_through() {
    let cfg = Config {
        mid_turn_compact_threshold: Some(0.80),
        ..Config::default()
    };
    assert_eq!(cfg.resolve_mid_turn_compact_threshold(), Some(0.80));
}

#[test]
fn mid_turn_threshold_upper_bound_inclusive() {
    let cfg = Config {
        mid_turn_compact_threshold: Some(1.0),
        ..Config::default()
    };
    assert_eq!(cfg.resolve_mid_turn_compact_threshold(), Some(1.0));
}

#[test]
fn mid_turn_threshold_out_of_range_treated_as_unset() {
    // Zero would compact constantly; negatives and >1 are nonsense. All map to
    // "unset" so a misconfigured value silently disables the feature rather
    // than wedging the agent.
    for bad in [0.0, -0.1, 1.5, 2.0] {
        let cfg = Config {
            mid_turn_compact_threshold: Some(bad),
            ..Config::default()
        };
        assert_eq!(
            cfg.resolve_mid_turn_compact_threshold(),
            None,
            "threshold {bad} should be treated as unset"
        );
    }
}

#[test]
fn compact_enabled_default_false() {
    assert!(!Config::default().resolve_compact_enabled());
}

#[test]
fn show_reasoning_defaults_off() {
    assert!(!Config::default().resolve_show_reasoning());
}

#[test]
fn show_reasoning_can_be_disabled() {
    let cfg = Config {
        show_reasoning: Some(false),
        ..Config::default()
    };
    assert!(!cfg.resolve_show_reasoning());
}

#[test]
fn context_exhausted_report_math() {
    // window 20000, threshold 0.80 -> ceiling 16000.
    // prompt 18000 -> 90% of window, overflow 18000 - 16000 = 2000.
    let lines = crate::ui::context_exhausted_report(18_000, 0.80, 20_000, 8_192, 6_000);
    let joined = lines.join("\n");
    assert!(
        joined.contains("context window .............. 20000 tokens"),
        "{joined}"
    );
    assert!(joined.contains("16000 tokens  (80% of window)"), "{joined}");
    assert!(joined.contains("18000 tokens  (90% of window)"), "{joined}");
    assert!(
        joined.contains("overflow above ceiling ...... 2000 tokens"),
        "{joined}"
    );
    assert!(
        joined.contains("reserved for response ....... 8192 tokens"),
        "{joined}"
    );
    assert!(
        joined.contains("kept-recent budget .......... 6000 tokens"),
        "{joined}"
    );
    // Guidance references the actual pressure and the floor the KV cache must hold.
    assert!(
        joined.contains("raise mid_turn_compact_threshold above 90%"),
        "{joined}"
    );
    assert!(joined.contains("hold 18000+ tokens"), "{joined}");
}

#[test]
fn catalog_context_window_reads_known_model() {
    // deepseek-v4-pro is a 1M-context model in the baked openrouter catalog.
    assert_eq!(
        Config::catalog_context_window("openrouter", "deepseek/deepseek-v4-pro"),
        Some(1_048_576)
    );
}

#[test]
fn catalog_context_window_none_for_unknown() {
    assert!(Config::catalog_context_window("openrouter", "no/such-model").is_none());
    // Providers without a baked catalog (custom gateways, ollama) return None.
    assert!(Config::catalog_context_window("ollama", "llama3.1").is_none());
}

#[test]
fn resolve_context_window_prefers_config_pin_over_catalog() {
    let cfg: Config = serde_json::from_str(r#"{ "context_window": 128000 }"#).unwrap();
    let qm = std::collections::HashMap::new();
    assert_eq!(
        cfg.resolve_context_window("openrouter", "deepseek/deepseek-v4-pro", &qm),
        128_000
    );
    // Without a pin, the catalog's 1M wins.
    let cfg = Config::default();
    assert_eq!(
        cfg.resolve_context_window("openrouter", "deepseek/deepseek-v4-pro", &qm),
        1_048_576
    );
}

#[test]
fn resolve_context_window_from_quick_model() {
    let mut qm = std::collections::HashMap::new();
    qm.insert(
        "test".to_string(),
        crate::config::types::QuickModelConfig {
            provider: compact_str::CompactString::new("openrouter"),
            model: compact_str::CompactString::new("deepseek/deepseek-chat"),
            input_token_cost: 0.0,
            output_token_cost: 0.0,
            reserve_tokens: None,
            temperature: None,
            extra_body: None,
            context_window: Some(64_000),
        },
    );
    let cfg = Config::default();
    // Quick model's 64k wins over the catalog's 128k for deepseek-chat.
    assert_eq!(
        cfg.resolve_context_window("openrouter", "deepseek/deepseek-chat", &qm),
        64_000
    );
    // Global config pin still wins over quick model.
    let cfg: Config = serde_json::from_str(r#"{ "context_window": 32000 }"#).unwrap();
    assert_eq!(
        cfg.resolve_context_window("openrouter", "deepseek/deepseek-chat", &qm),
        32_000
    );
    // Quick model with context_window: None falls through to catalog (128k).
    qm.get_mut("test").unwrap().context_window = None;
    let cfg = Config::default();
    let cw = cfg.resolve_context_window("openrouter", "deepseek/deepseek-chat", &qm);
    assert_eq!(cw, 128_000);
}

// ── YAML config reader (replaces the former JSON reader) ───────────────
//
// The on-disk config may be TOML or YAML. YAML is a strict superset of JSON,
// so legacy `config.json` files parse transparently through the YAML reader.
// These tests pin that contract: YAML parsing, the JSON-superset guarantee,
// round-tripping of `serde_json::Value` fields (extra_body / permission), and
// the filename resolution priority.

#[test]
fn yaml_reader_parses_config() {
    let yaml = r#"provider: openrouter
model: deepseek/deepseek-v4-flash
max_tokens: 16384
temperature: 0.7
context_window: 128000
compact_enabled: true
default_prompt: code
show_tool_details: 3
permission-modes: ["guarded", "standard", "yolo"]
mid_turn_compact_threshold: 0.80
quick_models:
  fast:
    provider: openai
    model: gpt-4o-mini
custom_providers:
  local-vllm:
    provider_type: openai
    base_url: http://localhost:8000/v1
    api_key_env: VLLM_API_KEY
permission:
  '*': ask
  read: allow
  bash:
    'cargo test': allow
    'rm **': deny
"#;
    let cfg: Config = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(cfg.provider.as_deref(), Some("openrouter"));
    assert_eq!(cfg.model.as_deref(), Some("deepseek/deepseek-v4-flash"));
    assert_eq!(cfg.max_tokens, Some(16384));
    assert_eq!(cfg.temperature, Some(0.7));
    assert_eq!(cfg.context_window, Some(128000));
    assert_eq!(cfg.compact_enabled, Some(true));
    assert_eq!(cfg.default_prompt.as_deref(), Some("code"));
    assert_eq!(cfg.mid_turn_compact_threshold, Some(0.80));
    match cfg.show_tool_details {
        Some(crate::config::ShowToolDetails::Lines(3)) => {}
        other => panic!("unexpected show_tool_details: {other:?}"),
    }
    assert_eq!(
        cfg.permission_modes.as_deref(),
        Some(
            &[
                "guarded".to_string(),
                "standard".to_string(),
                "yolo".to_string()
            ][..]
        )
    );
    let qm = cfg.quick_models.expect("quick_models");
    let fast = qm.get("fast").expect("fast model");
    assert_eq!(fast.provider.as_str(), "openai");
    assert_eq!(fast.model.as_str(), "gpt-4o-mini");
    let cps = cfg.custom_providers.expect("custom_providers");
    let vllm = cps.get("local-vllm").expect("local-vllm provider");
    assert_eq!(vllm.base_url, "http://localhost:8000/v1");
    assert_eq!(vllm.api_key_env.as_deref(), Some("VLLM_API_KEY"));
    assert_eq!(
        cfg.permission,
        Some(serde_json::json!({
            "*": "ask",
            "read": "allow",
            "bash": { "cargo test": "allow", "rm **": "deny" }
        }))
    );
}

#[test]
fn yaml_reader_accepts_plain_json_superset() {
    // YAML is a superset of JSON: a plain JSON config must parse through the
    // YAML reader identically to the equivalent YAML.
    let json = r#"{
      "provider": "openrouter",
      "model": "deepseek/deepseek-v4-flash",
      "max_tokens": 16384,
      "compact_enabled": true,
      "quick_models": {
        "fast": { "provider": "openai", "model": "gpt-4o-mini" }
      },
      "permission": { "*": "ask", "read": "allow" }
    }"#;
    let from_json: Config = serde_yaml_ng::from_str(json).unwrap();

    let yaml = r#"provider: openrouter
model: deepseek/deepseek-v4-flash
max_tokens: 16384
compact_enabled: true
quick_models:
  fast:
    provider: openai
    model: gpt-4o-mini
permission:
  '*': ask
  read: allow
"#;
    let from_yaml: Config = serde_yaml_ng::from_str(yaml).unwrap();

    assert_eq!(from_json.provider, from_yaml.provider);
    assert_eq!(from_json.model, from_yaml.model);
    assert_eq!(from_json.max_tokens, from_yaml.max_tokens);
    assert_eq!(from_json.compact_enabled, from_yaml.compact_enabled);
    let jf = from_json
        .quick_models
        .as_ref()
        .and_then(|m| m.get("fast"))
        .expect("json fast model");
    let yf = from_yaml
        .quick_models
        .as_ref()
        .and_then(|m| m.get("fast"))
        .expect("yaml fast model");
    assert_eq!(jf.provider.as_str(), yf.provider.as_str());
    assert_eq!(jf.model.as_str(), yf.model.as_str());
    assert_eq!(from_json.permission, from_yaml.permission);
}

#[test]
fn yaml_round_trips_serde_json_value_fields() {
    // `extra_body` and `permission` are typed as `serde_json::Value`; ensure
    // they survive a YAML serialize/deserialize round trip intact.
    let cfg = Config {
        provider: Some(CompactString::new("openrouter")),
        extra_body: Some(serde_json::json!({ "plugins": { "preset": "quality" } })),
        permission: Some(serde_json::json!({ "*": "ask", "read": "allow" })),
        ..Config::default()
    };
    let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
    let back: Config = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(back.provider, cfg.provider);
    assert_eq!(back.extra_body, cfg.extra_body);
    assert_eq!(back.permission, cfg.permission);
}

#[test]
fn yaml_round_trips_scalar_and_nested_fields() {
    let cfg = Config {
        provider: Some(CompactString::new("openrouter")),
        model: Some(CompactString::new("deepseek/deepseek-v4-flash")),
        max_tokens: Some(16384),
        context_window: Some(128000),
        compact_enabled: Some(true),
        default_prompt: Some(CompactString::new("code")),
        ..Config::default()
    };
    let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
    // The emitter produces block-style YAML, not JSON flow braces.
    assert!(!yaml.trim_start().starts_with('{'));
    let back: Config = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(back.provider, cfg.provider);
    assert_eq!(back.model, cfg.model);
    assert_eq!(back.max_tokens, cfg.max_tokens);
    assert_eq!(back.context_window, cfg.context_window);
    assert_eq!(back.compact_enabled, cfg.compact_enabled);
    assert_eq!(back.default_prompt, cfg.default_prompt);
}

// `pick_existing` is pure (no env/global state), so this priority test is
// hermetic and safe to run in parallel with everything else.
#[test]
fn config_candidate_priority_toml_yaml_yml_legacy_json() {
    use crate::config::load::pick_existing;

    let dir = std::env::temp_dir().join(format!("zs_cfgtest_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let name = |p: std::path::PathBuf| p.file_name().unwrap().to_str().unwrap().to_string();

    // Nothing exists yet -> defaults to the preferred config.toml path.
    assert_eq!(name(pick_existing(&dir)), "config.toml");

    // Legacy config.json is still discovered (parsed via the YAML reader, since
    // YAML is a superset of JSON).
    std::fs::write(dir.join("config.json"), "{}").unwrap();
    assert_eq!(name(pick_existing(&dir)), "config.json");

    // .yml outranks legacy .json.
    std::fs::write(dir.join("config.yml"), "").unwrap();
    assert_eq!(name(pick_existing(&dir)), "config.yml");
    let _ = std::fs::remove_file(dir.join("config.yml"));

    // .yaml outranks legacy .json.
    std::fs::write(dir.join("config.yaml"), "").unwrap();
    assert_eq!(name(pick_existing(&dir)), "config.yaml");

    // .yaml also outranks .yml when both exist.
    std::fs::write(dir.join("config.yml"), "").unwrap();
    assert_eq!(name(pick_existing(&dir)), "config.yaml");

    // .toml outranks every other candidate.
    std::fs::write(dir.join("config.toml"), "").unwrap();
    assert_eq!(name(pick_existing(&dir)), "config.toml");

    let _ = std::fs::remove_dir_all(&dir);
}
