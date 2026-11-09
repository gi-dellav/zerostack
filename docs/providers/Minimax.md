---
description: "Use MiniMax models with zerostack through a custom provider."
---

# MiniMax

MiniMax is available in zerostack through custom provider definitions. The
configuration below keeps the API protocol and region explicit while sharing a
single `MINIMAX_API_KEY` environment variable. New configurations default to
MiniMax-M3.1-Flash; MiniMax-M3 remains available as an alternative.

## Credentials

Set the API key in your shell instead of storing it in the zerostack config:

```bash
export MINIMAX_API_KEY="your-api-key"
```

## Provider Routes

Add the routes you need to `~/.config/zerostack/config.toml`:

```toml
[custom_providers.minimax-global-openai]
provider_type = "openai"
base_url = "https://api.minimax.io/v1"
api_key_env = "MINIMAX_API_KEY"
api_style = "completions"
model = "MiniMax-M3.1-Flash"

[custom_providers.minimax-cn-openai]
provider_type = "openai"
base_url = "https://api.minimaxi.com/v1"
api_key_env = "MINIMAX_API_KEY"
api_style = "completions"
model = "MiniMax-M3.1-Flash"

[custom_providers.minimax-global-anthropic]
provider_type = "anthropic"
base_url = "https://api.minimax.io/anthropic"
api_key_env = "MINIMAX_API_KEY"
model = "MiniMax-M3.1-Flash"

[custom_providers.minimax-cn-anthropic]
provider_type = "anthropic"
base_url = "https://api.minimaxi.com/anthropic"
api_key_env = "MINIMAX_API_KEY"
model = "MiniMax-M3.1-Flash"
```

Keep each Anthropic-compatible `base_url` ending in `/anthropic`. The
Anthropic client appends `/v1/messages` when it sends a request, so do not add
`/v1` to the configured base URL.

Select a route using its configured default, or explicitly select MiniMax-M3:

```bash
zerostack --provider minimax-global-openai
zerostack --provider minimax-cn-openai
zerostack --provider minimax-global-anthropic
zerostack --provider minimax-cn-anthropic --model MiniMax-M3
```

Existing saved models and explicit `--model` requests are retained, including
legacy IDs. Ordinary model pickers hide older MiniMax text models on the direct
API and known built-in gateway routes. Available Flash entries precede MiniMax-M3
without moving other providers' choices. Gateway IDs, availability, pricing and
transport come from the gateway catalog.
OpenCode Flash choices also require an entry in the bundled transport map; until
then, MiniMax-M3 remains its ordinary MiniMax choice.
Custom gateways with other hostnames retain their own model choices.

## Model Configuration

| Model | Context window | Max output | Input modes | Thinking | Input | Output | Cache read | Cache write |
| ----- | -------------: | ---------: | ----------- | -------- | ----: | -----: | ---------: | ----------: |
| `MiniMax-M3.1-Flash` | 1,000,000 | 524,288 | text, image, video | adaptive only | $0.15 | $0.60 | $0.015 | not listed |
| `MiniMax-M3` | 1,000,000 | — | text, image, video | adaptive or disabled | $0.60 | $2.40 | $0.12 | not listed |

Prices are in USD per million tokens. The MiniMax-M3 row uses the standard
full-context rate. Its service-tier pricing is:

| Service tier | Context | Input | Output | Cache read |
| ------------ | ------- | ----: | -----: | ---------: |
| standard | up to 512K tokens | $0.30 | $1.20 | $0.06 |
| standard | over 512K tokens | $0.60 | $2.40 | $0.12 |
| priority | up to 512K tokens | $0.45 | $1.80 | $0.09 |
| priority | over 512K tokens | $0.90 | $3.60 | $0.18 |

The optional quick-model entries below use the standard tier for their full
configured context windows in zerostack's local cost estimate:

```toml
[quick_models.minimax-m3-1-flash]
provider = "minimax-global-openai"
model = "MiniMax-M3.1-Flash"
context_window = 1000000
input_token_cost = 0.15
output_token_cost = 0.6

[quick_models.minimax-m3]
provider = "minimax-global-openai"
model = "MiniMax-M3"
context_window = 1000000
input_token_cost = 0.6
output_token_cost = 2.4
```

Switch with `--quick-model minimax-m3-1-flash` or `--quick-model minimax-m3`.
Change the `provider` value to use another route from the configuration above.

## Thinking and effort

Flash uses adaptive thinking by default on both protocols. Do not send
`thinking.type = "disabled"`, `reasoning_effort = "none"`, `service_tier`, or
`reasoning_split = false` for Flash. Supported effort values are `low`, `medium`,
`high`, `xhigh`, and `max` (default). To select an effort, add one of these inline
`extra_body` values inside the corresponding quick-model entry:

```toml
# OpenAI-compatible route:
extra_body = { reasoning_effort = "high" }
# Anthropic-compatible route (use instead of the line above):
# extra_body = { output_config = { effort = "high" } }
```

MiniMax-M3 supports adaptive or disabled thinking, defaults to adaptive on the
OpenAI-compatible route and disabled on the Anthropic-compatible route, and
ignores effort. Its thinking mode can be set using
`extra_body = { thinking = { type = "adaptive" } }` or `"disabled"`.
The TUI reasoning toggle controls zerostack's prompt/display behavior; it does
not replace these API parameters. Review any global `extra_body` settings before
switching models, since explicitly configured parameters are passed through.

See the [model overview](https://platform.minimax.io/docs/guides/models-intro),
[OpenAI-compatible API](https://platform.minimax.io/docs/api-reference/text-openai-api),
and [Anthropic-compatible API](https://platform.minimax.io/docs/api-reference/text-anthropic-api)
for current API capabilities.
