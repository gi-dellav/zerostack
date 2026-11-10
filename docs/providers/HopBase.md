---
description: "Use HopBase models with zerostack through an OpenAI-compatible custom provider."
---

# HopBase

[HopBase](https://hop-base.com) is an AI API gateway. One base URL serves
models from several families (for example GLM, GPT and Claude) over OpenAI
Chat Completions, OpenAI Responses and Anthropic Messages. Use zerostack's
custom provider configuration to connect to its Chat Completions endpoint.

## Credentials

Sign up and create an API key in the
[HopBase console](https://api.hop-base.com/login), then set it in your shell
instead of storing it in the config:

```bash
export HOPBASE_API_KEY="your-api-key"
```

Each API key belongs to one model group, so the models a key can call depend
on the group it was created in.

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.hopbase]
provider_type = "openai"
base_url = "https://api.hop-base.com/v1"
api_key_env = "HOPBASE_API_KEY"
api_style = "completions"
model = "glm-5.3-flash"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions rather than the Responses API.

Start a session with:

```bash
zerostack --provider hopbase --model glm-5.3-flash
```

For a short text-only connectivity check:

```bash
zerostack --provider hopbase --model glm-5.3-flash --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

## Model IDs and Metadata

Model IDs are bare names such as `glm-5.3-flash`, `gpt-5.6-luna` or
`claude-sonnet-5-5`. The model listing requires your key and returns only
the IDs that key can call:

```bash
curl https://api.hop-base.com/v1/models \
  -H "Authorization: Bearer $HOPBASE_API_KEY"
```

Use a returned ID unchanged with `--model`.

The listing does not report `context_length` or `pricing`, so zerostack
falls back to a `128000` context window; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).
Set `context_window` in the config (or per quick model) to match the model
you use.

An optional quick model gives a model a short name:

```toml
[quick_models.hb-luna]
provider = "hopbase"
model = "gpt-5.6-luna"
```

Select it with `zerostack --quick-model hb-luna`.

Docs: https://hop-base.com/en/docs
