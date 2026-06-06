use compact_str::CompactString;

use crate::cli::Cli;
use crate::config::{self, Config};
use crate::provider::AnyClient;

pub fn try_init(
    cfg: &Config,
    cli: &Cli,
    client: &AnyClient,
    provider: &CompactString,
    model: &CompactString,
) {
    let advisor_enabled = if cli.no_advisor {
        false
    } else {
        cfg.advisor.as_ref().is_some_and(|a| a.enabled)
    };

    if !advisor_enabled {
        return;
    }

    let adv_cfg = cfg.advisor.as_ref().unwrap();
    let qm = config::quick_models_map(cfg);

    let adv_pair: Option<(CompactString, CompactString)> =
        if let Some(ref cli_model) = cli.advisor_model {
            if let Some(q) = qm.get(cli_model.as_str()) {
                Some((q.provider.clone(), q.model.clone()))
            } else {
                Some((provider.clone(), CompactString::new(cli_model.as_str())))
            }
        } else if let Some(ref cfg_model) = adv_cfg.model {
            if let Some(q) = qm.get(cfg_model.as_str()) {
                Some((q.provider.clone(), q.model.clone()))
            } else {
                let prov = adv_cfg.provider.clone().unwrap_or_else(|| provider.clone());
                Some((prov, cfg_model.clone()))
            }
        } else if let Some(ref adv_prov) = adv_cfg.provider {
            Some((CompactString::new(adv_prov.as_str()), model.clone()))
        } else {
            tracing::warn!(
                "Advisor enabled but no advisor model configured. \
                 Set `advisor.model` in config or pass --advisor-model. Disabling advisor."
            );
            None
        };

    if let Some((adv_provider, adv_model)) = adv_pair {
        let adv_client = if adv_provider.as_str() == provider {
            client.clone()
        } else {
            match crate::provider::create_client(
                &adv_provider,
                cli.api_key.as_deref(),
                &cfg.custom_providers_map(),
                cfg.api_keys.as_ref(),
            ) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "Could not initialize advisor provider '{}' ({}); \
                         falling back to main provider '{}'.",
                        adv_provider,
                        e,
                        provider
                    );
                    client.clone()
                }
            }
        };

        super::init(adv_client, adv_model.to_string(), adv_cfg.max_turns);
    }
}
