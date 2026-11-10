---
description: "Use UberLLM models with zerostack through an OpenAI-compatible custom provider."
---

# UberLLM

[UberLLM](https://uberllm.dev) is an OpenAI-compatible LLM gateway. One prepaid
API key gives access to chat models from several labs (DeepSeek, Anthropic,
OpenAI, Google, Qwen and others). Use zerostack's custom provider configuration
to connect to its Chat Completions endpoint.

## Credentials

Create an API key at https://uberllm.dev/signup, then set it in your shell
instead of storing it in the config:

```bash
export UBERLLM_API_KEY="your-api-key"
```

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.uberllm]
provider_type = "openai"
base_url = "https://api.uberllm.dev/v1"
api_key_env = "UBERLLM_API_KEY"
api_style = "completions"
model = "deepseek/deepseek-v4.1-flash"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions rather than the Responses API.

Start a session with:

```bash
zerostack --provider uberllm --model deepseek/deepseek-v4.1-flash
```

For a short text-only connectivity check:

```bash
zerostack --provider uberllm --model deepseek/deepseek-v4.1-flash --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

## Model IDs and Metadata

Model IDs use the `vendor/model` form. The chat model listing is public, so no
key is needed to browse it:

```bash
curl https://api.uberllm.dev/v1/models
```

Use a returned ID unchanged with `--model`, for example
`deepseek/deepseek-v4.1-flash` or `anthropic/claude-sonnet-4.6`. Current
per-model prices are listed at https://uberllm.dev/models.

The listing reports OpenRouter-style `context_length` and `pricing` fields, so
zerostack reads the context window and token pricing live; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).
A `context_window` set in the config (or per quick model) overrides the live
value.

An optional quick model gives a second route a short name:

```toml
[quick_models.uberllm-sonnet]
provider = "uberllm"
model = "anthropic/claude-sonnet-4.6"
```

Select it with `zerostack --quick-model uberllm-sonnet`.

Docs: https://uberllm.dev/docs
