---
description: "Use OpenLLM with zerostack through an OpenAI-compatible custom provider, with the local daemon or the cloud endpoint."
---

# OpenLLM

[OpenLLM](https://www.openllm.sh) routes your API keys, OAuth subscription
accounts and local models through one format-agnostic endpoint. It runs locally
and extends to the cloud. It is available in zerostack through a custom
provider that uses the Chat Completions transport. See the
[OpenLLM documentation](https://docs.openllm.sh) for installation and sign-in.

## Local daemon

Install OpenLLM and sign in once (`curl -fsSL https://www.openllm.sh/install | bash`,
then `openllm login`). The daemon listens on `127.0.0.1:8787` and uses the
saved login, so the request needs no real key.

zerostack still requires some key value for an OpenAI-type custom provider, so
store a placeholder in the config. Add this to your zerostack config. See
[Configuration](../CONFIG.md) for config locations and project-local overrides.

```toml
[custom_providers.openllm]
provider_type = "openai"
base_url = "http://127.0.0.1:8787/v1"
api_style = "completions"
model = "plus"

[api_keys]
openllm = "local"
```

The daemon reads the placeholder as "use the saved login". A value that does
not start with `sk-llm-` is never treated as an OpenLLM key.

Start a session with:

```bash
zerostack --provider openllm --model plus
```

For a short text-only connectivity check:

```bash
zerostack --provider openllm --model lite --no-tools --max-tokens 32 -p "Reply with exactly pong."
```

If the daemon is not running, start it with `openllm start`. A `401` whose
message begins with `[openllm] API key required.` means the daemon has no saved
login: run `openllm login`.

## Cloud endpoint

For CI or a machine without the daemon, use the cloud origin and an OpenLLM API
key (`sk-llm-...`). Set it in your shell instead of storing it in the config:

```bash
export OPENLLM_API_KEY="your-api-key"
```

```toml
[custom_providers.openllm-cloud]
provider_type = "openai"
base_url = "https://www.openllm.sh/v1"
api_key_env = "OPENLLM_API_KEY"
api_style = "completions"
model = "plus"
```

```bash
zerostack --provider openllm-cloud --model plus
```

Models that come from a subscription are served by one of your online devices
through the relay. If none of your devices is online, those requests fail with
`403` (`subscription_requires_daemon` or `local_requires_daemon`) while models
served from the cloud keep working.

## Model IDs and Metadata

`ultra` (most capable), `plus` (everyday) and `lite` (fast, low cost) are
built-in chains. They exist for the providers you have connected, and you can
disable them, so check the listing first. To see the models available to you:

```bash
curl http://127.0.0.1:8787/v1/models
```

For the cloud endpoint, add `-H "Authorization: Bearer $OPENLLM_API_KEY"` and
use `https://www.openllm.sh/v1/models`. Chat models carry `chat` in
`capabilities`. Provider model IDs have the form `<provider>/<model>`, and the built-in
chains use bare IDs. Use a returned ID unchanged with `--model`.

The listing reports `context_window` rather than the OpenRouter-style
`context_length` that zerostack reads, so zerostack falls back to its default
context window for these models. Set `context_window` yourself, for example per
quick model, using the value from the listing; see
[Live context window and pricing](../PROVIDERS.md#live-context-window-and-pricing).

```toml
[quick_models.openllm-lite]
provider = "openllm"
model = "lite"
context_window = 128000
```

This optional quick model sets a conservative local budget. It is not a claim
about the model's maximum context. Select it with
`zerostack --quick-model openllm-lite`. A global `context_window` setting takes
precedence over the quick model's value.
