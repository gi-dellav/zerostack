---
description: "Use OpenCode Zen and Go models with zerostack, including keyless free models."
---

# OpenCode Zen / Go

[OpenCode Zen](https://opencode.ai/docs/zen) (pay-as-you-go) and
[OpenCode Go](https://opencode.ai/docs/go) ($10/month subscription) are
curated gateways of models benchmarked for coding agents. Both are built into
zerostack as first-class providers sharing a single `OPENCODE_API_KEY`
environment variable.

## Credentials

Set the API key in your shell instead of storing it in the zerostack config:

```bash
export OPENCODE_API_KEY="your-api-key"
```

Zen also serves free models (e.g. `mimo-v2.5-free`, `big-pickle`) with no key
at all: when `opencode-zen` has no configured key, zerostack sends the public
credential, which only the free models accept. Paid models reject it
server-side, so add a real key for anything else. Go has no free tier and
always requires a key.

Every request carries a stable per-conversation session id
(`x-opencode-session`, the zerostack session id) that the gateways use for
routing and prompt-cache affinity. The free tier rejects requests without it,
so zerostack always sends it — no configuration needed.

```bash
zerostack --provider opencode-zen --model mimo-v2.5-free
zerostack --provider opencode-zen --model kimi-k2.6
zerostack --provider opencode-go --model kimi-k2.6
```

## Model Configuration

One provider id covers three API shapes, and zerostack picks the right one
per model automatically (chat completions, responses, or Anthropic-native
messages), following each model's documented endpoint. The mapping is baked
into the binary and refreshed weekly from models.dev; run `/models refresh`
for the live list.

```toml
[quick_models.kimi-zen]
provider = "opencode-zen"
model = "kimi-k2.6"

[quick_models.sonnet-zen]
provider = "opencode-zen"
model = "claude-sonnet-4-6"
```

Switch with `--quick-model kimi-zen` or `/provider opencode-zen` followed by
`/model kimi-k2.6`.

## Known limitations

- Gemini models served through Zen (`gemini-*-flash`) are not in the baked
  catalog: zerostack only speaks the chat/responses/messages transports, not
  the Gemini-native endpoint. Use the direct `gemini` provider for those.
- A model added after the last catalog refresh falls back to chat
  completions with a warning. If the fallback transport is wrong the request
  fails loudly; point a manual `custom_providers` entry at the same base URL
  with an explicit `provider_type`/`api_style` as an escape hatch until the
  catalog refreshes.
