use crate::config::{ApiStyle, CustomProviderConfig};
use crate::provider::{AnyModel, OpenAiModel, create_client, expand_env, resolve_api_style};

fn cfg(api_style: Option<ApiStyle>) -> CustomProviderConfig {
    CustomProviderConfig {
        provider_type: "openai".into(),
        base_url: "https://gw.example/v1".to_string(),
        api_key_env: None,
        danger_accept_invalid_certs: None,
        api_style,
        headers: std::collections::HashMap::new(),
        timeout_secs: None,
        model: None,
    }
}

#[test]
fn defaults_to_responses_without_base_url() {
    assert_eq!(resolve_api_style(None, None), ApiStyle::Responses);
}

#[test]
fn defaults_to_completions_with_base_url() {
    assert_eq!(
        resolve_api_style(Some("https://gw.example/v1"), None),
        ApiStyle::Completions
    );
}

#[test]
fn explicit_style_overrides_base_url_heuristic() {
    let c = cfg(Some(ApiStyle::Responses));
    assert_eq!(
        resolve_api_style(Some("https://gw.example/v1"), Some(&c)),
        ApiStyle::Responses
    );
}

#[test]
fn explicit_completions_overrides_no_base_url() {
    let c = cfg(Some(ApiStyle::Completions));
    assert_eq!(resolve_api_style(None, Some(&c)), ApiStyle::Completions);
}

#[test]
fn expand_env_passthrough() {
    assert_eq!(expand_env("Bearer abc").unwrap(), "Bearer abc");
}

#[test]
fn expand_env_reads_var() {
    // SAFETY: test-only; set and removed within a single test
    unsafe { std::env::set_var("ZS_TEST_HDR", "secret-value") };
    assert_eq!(expand_env("${ZS_TEST_HDR}").unwrap(), "secret-value");
    unsafe { std::env::remove_var("ZS_TEST_HDR") };
}

#[test]
fn expand_env_missing_var_errors() {
    assert!(expand_env("${ZS_DEFINITELY_NOT_SET_98237}").is_err());
}

#[test]
fn copilot_gpt_5_models_use_openai_responses_api() {
    let client = create_client(
        "copilot",
        Some("tid=1;proxy-ep=proxy.individual.githubcopilot.com;exp=2"),
        &std::collections::HashMap::new(),
        None,
    )
    .unwrap();

    assert!(matches!(
        client.completion_model("gpt-5.5"),
        AnyModel::OpenAI(OpenAiModel::Responses(_))
    ));
}

#[test]
fn copilot_non_gpt_5_models_use_openai_completions_api() {
    let client = create_client(
        "copilot",
        Some("tid=1;proxy-ep=proxy.individual.githubcopilot.com;exp=2"),
        &std::collections::HashMap::new(),
        None,
    )
    .unwrap();

    assert!(matches!(
        client.completion_model("gpt-4.1"),
        AnyModel::OpenAI(OpenAiModel::Completions(_))
    ));
}
