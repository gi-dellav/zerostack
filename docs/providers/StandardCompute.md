---
description: "Use Standard Compute models with zerostack through an OpenAI-compatible custom provider."
---

# Standard Compute

[Standard Compute](https://standardcompute.com) is an OpenAI-compatible LLM
gateway. One API key gives access to models from several labs, plus a routed
model that picks a model for each request. Use zerostack's custom provider
configuration to connect to its Chat Completions endpoint.

## Credentials

Create an API key at https://standardcompute.com/dashboard, then set it in your
shell instead of storing it in the config:

```bash
export STANDARDCOMPUTE_API_KEY="your-api-key"
```

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.standardcompute]
provider_type = "openai"
base_url = "https://api.stdcmpt.com/v1"
api_key_env = "STANDARDCOMPUTE_API_KEY"
api_style = "completions"
model = "StandardCompute"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions rather than the Responses API.

Start a session with:

```bash
zerostack --provider standardcompute --model StandardCompute
```

For a short text-only connectivity check:

```bash
zerostack --provider standardcompute --model StandardCompute --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

## Model IDs and Metadata

`StandardCompute` is a routed model: each request goes to the cheapest enabled
model that can handle it, with automatic fallback. To pin one model instead,
use an exact ID from the public model listing:

```bash
curl https://api.stdcmpt.com/v1/models
```

Use a returned ID unchanged with `--model`, for example
`anthropic/claude-opus-5.5` or `google/gemini-3.8-flash`.

The listing reports an OpenRouter-style `context_length` field, so zerostack
reads the context window live; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).
A `context_window` set in the config (or per quick model) overrides the live
value.

An optional quick model gives a pinned route a short name:

```toml
[quick_models.sc-opus]
provider = "standardcompute"
model = "anthropic/claude-opus-5.5"
```

Select it with `zerostack --quick-model sc-opus`.

Docs: https://docs.standardcompute.com
