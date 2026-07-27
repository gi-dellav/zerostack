---
description: "Configure LLM providers in zerostack: OpenRouter, OpenAI-compatible endpoints, Anthropic, Gemini, Ollama, custom headers, and prompt caching."
---

# Providers

zerostack supports five built-in providers and allows custom provider
definitions for OpenAI-compatible endpoints.

## Built-in Providers

| Provider   | Config name         | Default env var for API key |
| ---------- | ------------------- | --------------------------- |
| OpenRouter | `openrouter`        | `OPENROUTER_API_KEY`        |
| OpenAI     | `openai`            | `OPENAI_API_KEY`            |
| Anthropic  | `anthropic`         | `ANTHROPIC_API_KEY`         |
| Gemini     | `gemini` / `google` | `GEMINI_API_KEY`            |
| Ollama     | `ollama`            | (no key required)           |

Select a provider via the config file, the `--provider` CLI flag, or the
`ZS_PROVIDER` environment variable:

```
zerostack --provider anthropic
```

The model is set with `--model` or `ZS_MODEL`:

```
zerostack --provider openai --model gpt-4o
```

## Provider Recipes

- [MiniMax](../providers/Minimax.md)

## Custom Providers

Custom providers let you point zerostack at any OpenAI-compatible API (vLLM,
LiteLLM, Ollama, local models, enterprise gateways, etc.). Define them under
the `custom_providers` key in the config file:

```json
{
  "custom_providers": {
    "local-vllm": {
      "provider_type": "openai",
      "base_url": "http://localhost:8000/v1",
      "api_key_env": "VLLM_API_KEY",
      "model": "gemma4"
    },
    "company-gateway": {
      "provider_type": "openai",
      "base_url": "https://gateway.example.com/v1",
      "model": "glm"
    }
  }
}
```

| Field                         | Type    | Description                                                                                                                                                                   |
| ----------------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `provider_type`               | string  | Must be one of the built-in provider types (`openrouter`, `openai`, `anthropic`, `gemini`, `ollama`).                                                                         |
| `base_url`                    | string  | The API base URL.                                                                                                                                                             |
| `api_key_env`                 | string  | Optional. Name of an environment variable holding the API key. Falls back to the provider-kind default if not set.                                                            |
| `api_style`                   | string  | Optional. For OpenAI-based providers: `"responses"` (Responses API, default when no `base_url` is set) or `"completions"` (Chat Completions, default when `base_url` is set). |
| `headers`                     | object  | Optional. HTTP headers to include in every request. Values support `${ENV_VAR}` expansion.                                                                                    |
| `danger_accept_invalid_certs` | boolean | Optional. Disables TLS certificate verification (MITM risk — use with care).                                                                                                  |
| `timeout_secs`                | integer | Optional. Overrides the default HTTP timeout.                                                                                                                                 |
| `model`                       | string  | Optional. Default model name for this provider. Used when no model is specified via `--model` or `ZS_MODEL`.                                                                  |

### Live context window and pricing

For custom providers, zerostack reads the context window and token pricing
live from `GET {base_url}/models` when the endpoint reports OpenRouter-style
`context_length` / `pricing` fields (pricing may be strings, as on OpenRouter,
or plain numbers). This drives the statusline context meter, auto-compaction,
and cost tracking. Providers whose listing omits these fields fall back to
`128000`; set `context_window` in the config (or per quick model) to override —
config values always win over live discovery.

### Header variable expansion

Header values can reference environment variables with `${VAR}` syntax:

```json
{
  "custom_providers": {
    "company-gateway": {
      "provider_type": "openai",
      "base_url": "https://gateway.example.com/v1",
      "headers": {
        "cf-access-client-id": "${CF_ACCESS_CLIENT_ID}",
        "cf-access-client-secret": "${CF_ACCESS_CLIENT_SECRET}"
      }
    }
  }
}
```

## API Key Resolution

The API key is resolved in this priority order:

1. **CLI flag** `--api-key` (visible in process listings — use with care)
2. **Environment variable** — either the custom one from `api_key_env`, or the
   default env var for the provider kind
3. **Config file** `api_keys` map — keyed by provider slug or custom provider name
4. **Ollama** — returns an empty string (no key required)

### Config-level API keys

```json
{
  "api_keys": {
    "openai": "sk-...",
    "anthropic": "sk-ant-..."
  }
}
```

## OpenAI API Styles

The OpenAI provider supports two API transports:

- **Responses API** (`/responses`) — the default for OpenAI's own API. Required
  for GPT-5-series models that reject `max_tokens` on Chat Completions.
- **Chat Completions API** (`/chat/completions`) — the default when a custom
  `base_url` is set, since most OpenAI-compatible gateways implement only this
  endpoint.

Override with `api_style: "responses"` or `api_style: "completions"` on a
custom provider, or set `api_style` on the built-in OpenAI provider to force a
specific transport.

## Prompt caching

zerostack enables prompt caching automatically where the underlying rig provider supports it. The behavior depends on which provider backs the model you choose.

### Automatic — no zerostack action

These providers cache server-side without any markers in the request:

- **OpenAI** — automatic above ~1024 tokens, 50% discount on cached input.
- **Google Gemini 2.5+** — implicit caching, 75% discount.
- **DeepSeek** — automatic, persistent across days, ~90% discount.
- **xAI / Grok** — automatic, 75% discount; benefits from setting an `x-grok-conv-id` header which zerostack does not currently send.
- **Moonshot, Groq (Kimi K2)** — automatic.

For these providers, zerostack passes through to rig without additional configuration.

### Explicitly enabled by zerostack

These providers require `cache_control` markers; zerostack adds them via rig's `.with_prompt_caching()`:

- **Anthropic (direct API)** — marks system prompt, the final tool definition, and the last message. All three breakpoints contribute to cumulative savings as the conversation grows.
- **Claude via OpenRouter** — marks the system prompt only. Anthropic's caching is prefix-based, so the tools array in front of the system block is also captured. For `anthropic/*` model IDs, zerostack also pins `provider.order = ["Anthropic"]` with `allow_fallbacks: true`, because Bedrock and Vertex AI silently drop `cache_control` markers.

### Empirical impact

Measured on Sonnet 4.6, second turn of a tool-heavy session (grep + read across the zerostack repo, ~6k-token system prompt including AGENTS.md and ARCHITECTURE.md):

| Configuration               | turn 2 cost | reduction |
| --------------------------- | ----------: | --------: |
| Baseline (no caching)       |      $0.186 |         — |
| Anthropic + caching         |      $0.024 |      -87% |
| OpenRouter Claude + caching |      $0.026 |      -86% |

Projected monthly cost at 50 such turns per working day: $204 (baseline) → $26 (Anthropic direct) or $29 (OpenRouter Claude). The two cached paths are within ~$3/month of each other.

### Known limitation: OpenRouter does not mark the last message

As of rig 0.38, OpenRouter's `apply_prompt_caching` marks the system message only. The Anthropic provider also marks the last message, which means accumulated tool results from earlier turns continue to be cached as the conversation grows; OpenRouter does not mark this position.

On tool-heavy workloads this manifests as ~2,676 tokens running at full input rate on OpenRouter vs ~4 tokens on Anthropic direct, a gap of ~8% per turn. The bulk of savings comes from caching the system prompt and tools — both paths capture that.

This is an upstream rig limitation, not zerostack-specific.

**Recommendation:** if you happen to have both API keys, Anthropic direct is marginally cheaper. If you don't, OpenRouter Claude with caching captures most of the savings.

## CLI Flags

| Flag                | Env var       | Description                         |
| ------------------- | ------------- | ----------------------------------- |
| `--provider`        | `ZS_PROVIDER` | Provider name                       |
| `--model`           | `ZS_MODEL`    | Model name                          |
| `--quick-model`     | —             | Use a named quick model from config |
| `--api-key`         | —             | API key (visible in `ps`)           |
| `--max-tokens`      | —             | Maximum response tokens             |
| `--temperature`     | —             | Model temperature (0.0–2.0)         |
| `--max-agent-turns` | —             | Maximum agent turns per response    |
