---
description: "Use TeamoRouter models with zerostack through an OpenAI-compatible custom provider."
---

# TeamoRouter

[TeamoRouter](https://teamorouter.com) is an OpenAI-compatible LLM gateway.
One API key gives access to chat models from several labs (GLM, Kimi, DeepSeek,
Qwen, Claude, GPT, Gemini and others). Use zerostack's custom provider
configuration to connect to its Chat Completions endpoint.

## Credentials

Create an API key at https://teamorouter.com/dashboard?tab=api-keys, then set it
in your shell instead of storing it in the config:

```bash
export TEAMOROUTER_API_KEY="your-api-key"
```

## Provider Configuration

Add this table to your zerostack config. See [Configuration](../CONFIG.md)
for config locations and project-local overrides.

```toml
[custom_providers.teamorouter]
provider_type = "openai"
base_url = "https://api.teamorouter.com/v1"
api_key_env = "TEAMOROUTER_API_KEY"
api_style = "completions"
model = "glm-5.3"
```

Keep `base_url` ending in `/v1`. The client appends `/chat/completions`, so
do not include that suffix in the configured URL. `api_style = "completions"`
explicitly selects Chat Completions rather than the Responses API.

Start a session with:

```bash
zerostack --provider teamorouter --model glm-5.3
```

For a short text-only connectivity check:

```bash
zerostack --provider teamorouter --model glm-5.3 --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

## Model IDs and Metadata

Model IDs are bare names such as `glm-5.3`, `deepseek-v4-pro`, `kimi-k3.1-pro`
or `claude-sonnet-5-5`. The model listing requires your key:

```bash
curl https://api.teamorouter.com/v1/models \
  -H "Authorization: Bearer $TEAMOROUTER_API_KEY"
```

Use a returned ID unchanged with `--model`. Current per-model prices are
listed at https://teamorouter.com/models.

The listing does not report `context_length` or `pricing`, so zerostack
falls back to a `128000` context window; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).
Set `context_window` in the config (or per quick model) to match the model
you use.

An optional quick model gives a second model a short name:

```toml
[quick_models.teamo-sonnet]
provider = "teamorouter"
model = "claude-sonnet-5-5"
```

Select it with `zerostack --quick-model teamo-sonnet`.
