---
description: "Use TrustedRouter models with zerostack through an OpenAI-compatible custom provider."
---

# TrustedRouter

[TrustedRouter](https://trustedrouter.com) is an OpenAI-compatible LLM gateway.
One API key gives access to chat models from several labs (DeepSeek, Anthropic,
OpenAI, Google, Qwen and others). Use zerostack's custom provider configuration
to connect to its Chat Completions endpoint.

## Credentials

Create an API key at https://trustedrouter.com/console/api-keys, then set it in your shell
instead of storing it in the config:

```bash
export TRUSTEDROUTER_API_KEY="your-api-key"
```

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.trustedrouter]
provider_type = "openai"
base_url = "https://api.trustedrouter.com/v1"
api_key_env = "TRUSTEDROUTER_API_KEY"
api_style = "completions"
model = "deepseek/deepseek-v4-flash"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions rather than the Responses API.

Start a session with:

```bash
zerostack --provider trustedrouter --model deepseek/deepseek-v4-flash
```

For a short text-only connectivity check:

```bash
zerostack --provider trustedrouter --model deepseek/deepseek-v4-flash --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

## Model IDs and Metadata

Model IDs use the `vendor/model` form. The chat model listing is public, so no
key is needed to browse it:

```bash
curl https://api.trustedrouter.com/v1/models
```

Use a returned ID unchanged with `--model`, for example
`deepseek/deepseek-v4-flash` or `anthropic/claude-sonnet-4.5`. Current
per-model prices are listed at https://trustedrouter.com/models.

The listing reports OpenRouter-style `context_length` and `pricing` fields, so
zerostack reads the context window and token pricing live; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).
A `context_window` set in the config (or per quick model) overrides the live
value.

`trustedrouter/auto` picks a route per request, so its price varies with the
route chosen; use a fixed model ID when you want predictable cost tracking.

An optional quick model gives a second route a short name:

```toml
[quick_models.trustedrouter-sonnet]
provider = "trustedrouter"
model = "anthropic/claude-sonnet-4.5"
```

Select it with `zerostack --quick-model trustedrouter-sonnet`.

Docs: https://trustedrouter.com/docs
