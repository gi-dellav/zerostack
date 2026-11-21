---
description: "Use AIGROW models with zerostack through an OpenAI-compatible custom provider."
---

# AIGROW

[AIGROW](https://aigrow.gg/ai-api) is a prepaid API for Claude, GPT, Gemini,
Grok, Qwen and Kimi models. One API key and one base URL serve every model over
OpenAI Chat Completions; the GPT models are also served over the OpenAI
Responses API. Use zerostack's custom provider configuration to connect to
either endpoint.

## Credentials

Create an API key in the [AIGROW dashboard](https://aigrow.gg/dashboard/api),
then set it in your shell instead of storing it in the config:

```bash
export AIGROW_API_KEY="your-api-key"
```

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.aigrow]
provider_type = "openai"
base_url = "https://aigrow.gg/v1"
api_key_env = "AIGROW_API_KEY"
api_style = "completions"
model = "claude-haiku-5.5"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions, which serves every AIGROW model.

Start a session with:

```bash
zerostack --provider aigrow --model claude-haiku-5.5
```

For a short text-only connectivity check:

```bash
zerostack --provider aigrow --model claude-haiku-5.5 --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

### GPT models over the Responses API

The GPT models (for example `gpt-6.2-sol` and `gpt-6.1-sol`) can also run over
the Responses API. Add a second table with `api_style = "responses"`; the
client appends `/responses` to the same base URL:

```toml
[custom_providers.aigrow-responses]
provider_type = "openai"
base_url = "https://aigrow.gg/v1"
api_key_env = "AIGROW_API_KEY"
api_style = "responses"
model = "gpt-6.2-sol"
```

```bash
zerostack --provider aigrow-responses --model gpt-6.2-sol
```

Requests are stateless (`store` is always false), and function tools are
supported, so zerostack's built-in tools work on this route. Keep the
`aigrow` table for Claude, Gemini, Grok, Qwen and Kimi models.

## Model IDs and Metadata

Model IDs are plain slugs without a vendor prefix. The model listing is
public, so no key is needed to browse it:

```bash
curl https://aigrow.gg/v1/models
```

Use a returned ID unchanged with `--model`, for example `claude-haiku-5.5`,
`claude-sonnet-5.5`, `gpt-6.2-sol` or `gemini-4-neon`. The keyless listing
also shows each model's `context_length` and per-token `pricing`.

zerostack sends the key when it reads the listing, and the authenticated
listing reports only `context_window`, so the live lookup described in
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing)
finds no `context_length` and zerostack falls back to `128000`. Set
`context_window` in the config (or per quick model) to the value the public
listing shows for your model, for example `1000000` for `claude-haiku-5.5`.

An optional quick model gives a second route a short name:

```toml
[quick_models.aigrow-sonnet]
provider = "aigrow"
model = "claude-sonnet-5.5"
context_window = 1000000
```

Select it with `zerostack --quick-model aigrow-sonnet`.

Docs: https://aigrow.gg/ai-api/guide
