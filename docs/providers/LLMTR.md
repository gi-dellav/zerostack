---
description: "Use LLMTR models with zerostack through an OpenAI-compatible custom provider."
---

# LLMTR

[LLMTR](https://llmtr.com) is a prepaid OpenAI-compatible LLM gateway. One API
key gives access to chat models from several labs (Qwen, Anthropic, OpenAI,
Google, DeepSeek and others). Use zerostack's custom provider configuration to
connect to its Chat Completions endpoint.

## Credentials

Create an API key at https://llmtr.com/dashboard/keys, then set it in your shell
instead of storing it in the config:

```bash
export LLMTR_API_KEY="your-api-key"
```

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.llmtr]
provider_type = "openai"
base_url = "https://llmtr.com/v1"
api_key_env = "LLMTR_API_KEY"
api_style = "completions"
model = "qwen/qwen3.8-flash"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions rather than the Responses API.

Start a session with:

```bash
zerostack --provider llmtr --model qwen/qwen3.8-flash
```

For a short text-only connectivity check:

```bash
zerostack --provider llmtr --model qwen/qwen3.8-flash --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

## Model IDs and Metadata

Model IDs use the `vendor/model` form. The model listing is public, so no key
is needed to browse it:

```bash
curl https://llmtr.com/v1/models
```

Use a returned ID unchanged with `--model`, for example `qwen/qwen3.8-flash`,
`anthropic/claude-sonnet-5.5` or `deepseek/deepseek-v4-pro`.

The listing reports OpenRouter-style `context_length` and `pricing` fields, so
zerostack reads the context window and token pricing live; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).
A `context_window` set in the config (or per quick model) overrides the live
value.

An optional quick model gives a second route a short name:

```toml
[quick_models.llmtr-sonnet]
provider = "llmtr"
model = "anthropic/claude-sonnet-5.5"
```

Select it with `zerostack --quick-model llmtr-sonnet`.

Docs: https://llmtr.com/docs
