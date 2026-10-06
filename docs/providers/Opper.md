---
description: "Use Opper models with zerostack through an OpenAI-compatible custom provider."
---

# Opper

Opper is an EU-hosted AI gateway with 700+ models from 50+ providers behind one
OpenAI-compatible API. It is available in zerostack through custom provider
definitions. The configuration below keeps the API protocol explicit while
sharing a single `OPPER_API_KEY` environment variable.

## Credentials

Create an API key at https://platform.opper.ai and set it in your shell instead
of storing it in the zerostack config:

```bash
export OPPER_API_KEY="your-api-key"
```

## Provider Routes

Add the routes you need to `~/.config/zerostack/config.toml`:

```toml
[custom_providers.opper]
provider_type = "openai"
base_url = "https://api.opper.ai/v3/compat"
api_key_env = "OPPER_API_KEY"
api_style = "completions"
model = "claude-sonnet-4-6"

[custom_providers.opper-anthropic]
provider_type = "anthropic"
base_url = "https://api.opper.ai/v3/compat"
api_key_env = "OPPER_API_KEY"
model = "claude-sonnet-4-6"
```

Keep both `base_url` values ending in `/v3/compat`. The OpenAI client appends
`/chat/completions` and the Anthropic client appends `/v1/messages` when it
sends a request, so do not add `/v1` to the configured base URL.

Select any route and model with the standard CLI flags:

```bash
zerostack --provider opper --model claude-sonnet-4-6
zerostack --provider opper --model gemini-3.8-flash
zerostack --provider opper-anthropic --model claude-sonnet-4-6
```

## Model Configuration

Bare model names such as `claude-sonnet-4-6` are pools: Opper picks the
provider route for each request. To pin a route, use a `provider/model` id such
as `anthropic/claude-sonnet-4-6`. The full list of model ids is at
https://opper.ai/models.

| Model | Context window |
| ----- | -------------: |
| `claude-sonnet-4-6` | 1,000,000 |
| `gemini-3.8-flash` | 1,048,576 |

Token rates are the model providers' rates with no markup. See
https://opper.ai/pricing for details.

The optional quick-model entries below set the context window for each model:

```toml
[quick_models.opper-sonnet]
provider = "opper"
model = "claude-sonnet-4-6"
context_window = 1000000

[quick_models.opper-flash]
provider = "opper"
model = "gemini-3.8-flash"
context_window = 1048576
```

Switch to either entry with `--quick-model opper-sonnet` or
`--quick-model opper-flash`. Change the `provider` value in a quick-model entry
to use another route from the configuration above.
