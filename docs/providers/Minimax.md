# MiniMax

MiniMax is available in zerostack through custom provider definitions. The
configuration below keeps the API protocol and region explicit while sharing a
single `MINIMAX_API_KEY` environment variable.

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
model = "MiniMax-M3"

[custom_providers.minimax-cn-openai]
provider_type = "openai"
base_url = "https://api.minimaxi.com/v1"
api_key_env = "MINIMAX_API_KEY"
api_style = "completions"
model = "MiniMax-M3"

[custom_providers.minimax-global-anthropic]
provider_type = "anthropic"
base_url = "https://api.minimax.io/anthropic"
api_key_env = "MINIMAX_API_KEY"
model = "MiniMax-M3"

[custom_providers.minimax-cn-anthropic]
provider_type = "anthropic"
base_url = "https://api.minimaxi.com/anthropic"
api_key_env = "MINIMAX_API_KEY"
model = "MiniMax-M3"
```

Keep each Anthropic-compatible `base_url` ending in `/anthropic`. The
Anthropic client appends `/v1/messages` when it sends a request, so do not add
`/v1` to the configured base URL.

Select any route and model with the standard CLI flags:

```bash
zerostack --provider minimax-global-openai --model MiniMax-M3
zerostack --provider minimax-cn-openai --model MiniMax-M2.7
zerostack --provider minimax-global-anthropic --model MiniMax-M3
zerostack --provider minimax-cn-anthropic --model MiniMax-M2.7
```

## Model Configuration

| Model | Context window | Input modes | Thinking | Input | Output | Cache read | Cache write |
| ----- | -------------: | ----------- | -------- | ----: | -----: | ---------: | ----------: |
| `MiniMax-M3` | 1,000,000 | text, image, video | adaptive or disabled | $0.30 | $1.20 | $0.06 | not listed |
| `MiniMax-M2.7` | 204,800 | text | always on | $0.30 | $1.20 | $0.06 | $0.375 |

Prices are in USD per million tokens. MiniMax-M3 also has the following
service-tier pricing:

| Service tier | Context | Input | Output | Cache read |
| ------------ | ------- | ----: | -----: | ---------: |
| standard | up to 512,000 tokens | $0.30 | $1.20 | $0.06 |
| standard | over 512,000 tokens | $0.60 | $2.40 | $0.12 |
| priority | up to 512,000 tokens | $0.45 | $1.80 | $0.09 |
| priority | over 512,000 tokens | $0.90 | $3.60 | $0.18 |

The optional quick-model entries below use the standard tier at up to 512,000
tokens for zerostack's local cost estimate:

```toml
[quick_models.minimax-m3]
provider = "minimax-global-openai"
model = "MiniMax-M3"
context_window = 1000000
input_token_cost = 0.3
output_token_cost = 1.2

[quick_models.minimax-m2-7]
provider = "minimax-global-openai"
model = "MiniMax-M2.7"
context_window = 204800
input_token_cost = 0.3
output_token_cost = 1.2
```

Switch to either entry with `--quick-model minimax-m3` or
`--quick-model minimax-m2-7`. Change the `provider` value in a quick-model
entry to use another route from the configuration above.
