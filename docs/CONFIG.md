# Configuration

zerostack reads an optional config file. It supports both JSON and TOML
formats. The file is resolved by priority:

- If `ZS_CONFIG_DIR` is set: `$ZS_CONFIG_DIR/config.toml` or `$ZS_CONFIG_DIR/config.json`
- Otherwise: `~/.config/zerostack/config.toml` or `~/.config/zerostack/config.json`
- Otherwise: `~/.local/share/zerostack/config.toml` or `~/.local/share/zerostack/config.json`

If a `config.toml` exists at a higher priority, it is used. If neither exists
at any priority, a default `config.toml` is created in the lowest-priority
directory (`~/.local/share/zerostack/`). On macOS the XDG config path above
resolves to `~/Library/Application Support/zerostack/`.

Prompts and themes are loaded from the same data directory:

- Prompts: `~/.local/share/zerostack/prompts/`
- Themes: `~/.local/share/zerostack/themes/`

If `ZS_CONFIG_DIR` is set, it overrides the data directory for the config file
location only (prompts and themes still use `ZS_DATA_DIR` / the default data
dir). Set `ZS_CONFIG_DIR` when you want the config in a separate path from the
data files.

All config keys are optional. CLI flags and their environment-backed values
(such as `ZS_PROVIDER` and `ZS_MODEL`) take precedence where both exist.

Example (JSON):

```json
{
  "provider": "openrouter",
  "model": "deepseek/deepseek-v4-flash",
  "max_tokens": 16384,
  "temperature": 0.7,
  "context_window": 128000,
  "reserve_tokens": 8192,
  "keep_recent_tokens": 10000,
  "compact_enabled": true,
  "mid_turn_compact_threshold": 0.80,
  "deny_repeated_reads": false,
  "default_prompt": "code",
  "default_permission_mode": "standard",
  "permission-modes": ["guarded", "standard", "yolo"],
  "show_tool_details": 3,
  "sandbox": false,
  "quick_models": {
    "fast": {
      "provider": "openai",
      "model": "gpt-4o-mini"
    }
  },
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
      "api_key_env": "GATEWAY_API_KEY",
      "api_style": "completions",
      "headers": {
        "cf-access-client-id": "${CF_ACCESS_CLIENT_ID}",
        "cf-access-client-secret": "${CF_ACCESS_CLIENT_SECRET}"
      },
      "danger_accept_invalid_certs": false,
      "timeout_secs": 60
    }
  },
  "permission": {
    "*": "ask",
    "read": "allow",
    "write": {
      "**/*.rs": "allow",
      "**": "ask"
    },
    "bash": {
      "cargo test": "allow",
      "rm **": "deny"
    },
    "external_directory": {
      "/tmp/**": "allow",
      "/**": "ask"
    },
    "doom_loop": "ask"
  }
}
```

The same config in TOML:

```toml
provider = "openrouter"
model = "deepseek/deepseek-v4-flash"
max_tokens = 16384
temperature = 0.7
context_window = 128000
reserve_tokens = 8192
keep_recent_tokens = 10000
compact_enabled = true
mid_turn_compact_threshold = 0.80
edit_system = "similarity"
default_prompt = "code"
default_permission_mode = "standard"
permission-modes = ["guarded", "standard", "yolo"]
show_tool_details = 3

[quick_models.fast]
provider = "openai"
model = "gpt-4o-mini"

[custom_providers.local-vllm]
provider_type = "openai"
base_url = "http://localhost:8000/v1"
api_key_env = "VLLM_API_KEY"

[permission]
"*" = "ask"
read = "allow"

[permission.write]
"**/*.rs" = "allow"
"**" = "ask"

[permission.bash]
"cargo test" = "allow"
"rm **" = "deny"

[permission.external_directory]
"/tmp/**" = "allow"
"/**" = "ask"

permission.doom_loop = "ask"
```

Accepted top-level keys:

| Key                       | Type    | Description                                                                                                                                                                 |
| ------------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `provider`                | string  | Provider name. Built-ins are `openrouter`, `openai`, `anthropic`, `gemini`/`google`, and `ollama`; custom provider aliases are also accepted. Default: `openrouter`.        |
| `model`                   | string  | Model name. Default: `deepseek/deepseek-v4-flash`.                                                                                                                          |
| `max_tokens`              | integer | Maximum response tokens. Default: `16384`.                                                                                                                                  |
| `max_agent_turns`         | integer | Maximum agent turns per response. Default: `200`.                                                                                                                           |
| `temperature`             | number  | Model temperature value. Only configurable via the `--temperature` CLI flag (`0.0` to `2.0`). Config-file value is parsed but not currently applied.                        |
| `no_tools`                | boolean | Disable all tools. Default: `false`.                                                                                                                                        |
| `no_context_files`        | boolean | Disable loading global/project `AGENTS.md`, `CLAUDE.md`, and `ARCHITECTURE.md` (if `archmd` feature enabled) context files. Default: `false`.                               |
| `context_window`          | integer | Session context-window size used for status and auto-compaction. When unset, auto-detected from the selected model's catalog entry; falls back to `128000` if the model is not in the catalog. A value of `0` disables auto-compaction. |
| `reserve_tokens`          | integer | Tokens to reserve before compaction is triggered. When unset globally, falls back to the active quick model's `reserve_tokens` field, then to the hardcoded default of `8192`.                                                                                                         |
| `keep_recent_tokens`      | integer | Approximate recent-token budget kept verbatim during compaction. Default: `10000`.                                                                                          |
| `max_text_file_size`      | integer | Maximum allowed file size in bytes for read/write tool operations. Default: `1048576` (1 MB).                                                                               |
| `deny_repeated_reads`     | boolean | Block repeated reads of the same file section within a session until the file is edited or written. Default: `true`. Set to `false` to allow re-reading.                     |
| `compact_enabled`         | boolean | Master switch for all automatic conversation compaction (both between-turn and mid-turn). Default: `true`. When `false`, nothing is ever compacted automatically.            |
| `mid_turn_compact_threshold` | number | Opt-in mid-turn compaction. Fraction of the context window (`0.0`–`1.0`) of real provider prompt pressure at which to compact *during* a turn, not just between turns. Unset by default, meaning no mid-turn compaction. Honored only when `compact_enabled` is `true`. Recommended starting value: `0.80`. See Mid-turn compaction below.            |
| `always_show_welcome`     | boolean | Always show the welcome banner on startup, bypassing the one-shot marker file. Default: `false`.                                                                               |
| `auto-update-prompts`     | boolean | When `true`, always regenerate prompts on version change without asking. When `false`, never regenerate. When unset, asks interactively.                                         |
| `auto-update-themes`      | boolean | When `true`, always regenerate themes on version change without asking. When `false`, never regenerate. When unset, asks interactively.                                         |
| `edit_system`             | string  | Edit system mode: `"similarity"` (SEARCH/REPLACE with fuzzy matching, default) or `"hashedit"` (CRC-32 tag-based CAS edits). See Edit System Modes below.                     |
| `custom_providers`        | object  | Map of provider aliases to `{ "provider_type", "base_url", "api_key_env", "api_style", "headers", "danger_accept_invalid_certs", "timeout_secs" }`. `provider_type` must resolve to a built-in provider type; `api_key_env` is optional. For OpenAI providers, `api_style` selects `"responses"` or `"completions"`, `headers` sets custom HTTP headers (values support `${ENV_VAR}` expansion), and `timeout_secs` overrides the HTTP timeout. `danger_accept_invalid_certs` disables TLS verification. See the OpenAI API styles section below. |
| `permission`              | object  | Permission rules using glob patterns; see the permission config notes below.                                |
| `permission-regex`        | object  | Same structure as `permission` but patterns are interpreted as regex instead of glob.                       |
| `permission-allow`        | object  | Map of tool names to lists of glob patterns to allow. Works alongside the `permission` field. See below.    |
| `permission-ask`          | object  | Map of tool names to lists of glob patterns to prompt on. Works alongside the `permission` field. See below.|
| `permission-deny`         | object  | Map of tool names to lists of glob patterns to deny. Works alongside the `permission` field. See below.     |
| `restrictive`             | boolean | Select restrictive permission mode (ask for every operation). Overridden by `accept_all`/`yolo` if those are also true.                                                     |
| `accept_all`              | boolean | Select standard permission mode with auto-allow within CWD (equivalent to `default_permission_mode = "standard"`). Overridden by `yolo` if true.                            |
| `yolo`                    | boolean | Select yolo mode (allow all, ask for destructive bash commands).                                                                                                            |
| `permission-modes`        | array   | List of mode names that apply config-based rules. Default: `["guarded", "standard", "yolo"]`. Modes excluded from this list skip config rule matching entirely.             |
| `sandbox`                 | boolean | Run bash commands in the bubblewrap sandbox. Default: `false`.                                                                                                              |
| `default_permission_mode` | string  | Permission mode when no mode boolean/CLI flag is set. Accepts: `standard` (default), `restrictive`, `readonly`, `guarded`, `yolo`.                                          |
| `show_tool_details`       | boolean or integer | Show tool-result previews in the TUI. `false` hides output, `true` shows all lines, an integer limits to that many lines (e.g. `3`). Default: `3`. |
| `default_prompt`          | string  | Prompt name to activate on startup. Default: `code`. If the prompt file has a `%%mode=<mode>` first-line directive, the security mode is set automatically (see Prompt directives below). |
| `editor`                  | string  | Editor command for `Ctrl+G` (default: `$EDITOR` env var, then `editor`, then `nano`).                                                                                        |
| `api_keys`                | object  | Map of provider names to API keys (e.g. `"openai": "sk-..."`). Used as fallback when the corresponding env var is not set.                                                   |
| `quick_models`            | object  | Map of quick-model names to `{ "provider", "model", "reserve_tokens"?, "input_token_cost"?, "output_token_cost"? }`. Can be switched with `/models <name>` or `--quick-model=<name>`.                                                      |
| `mcp_servers`             | object  | MCP server map when compiled with the `mcp` feature. When omitted, recommended MCPs are auto-configured (see below).                                                   |
| `enable-exa-mcp`          | boolean | Auto-configure the Exa Web Search MCP server. Default: `true`.                                                                                                         |
| `enable-context7-mcp`     | boolean | Auto-configure the Context7 MCP server. Default: `false`.                                                                                                              |
| `enable-grepapp-mcp`      | boolean | Auto-configure the Grep.app MCP server. Default: `false`.                                                                                                              |
| `allow_all_mcp_calls`     | boolean | When `true`, permission checks are skipped for all MCP tool calls. Default: `false`.                                                                                   |
| `acp_servers`             | object  | ACP server config map when compiled with the `acp` feature. See the ACP section below.                                                                                       |
| `acp_host`                | string  | TCP bind host for ACP server mode (equivalent to `--acp-host`).                                                                                                              |
| `acp_port`                | integer | TCP bind port for ACP server mode (equivalent to `--acp-port`, default: 7243).                                                                                               |
| `colors`                  | object  | Background color overrides for the TUI. See the colors section below.                                                                                                       |

## Mid-turn compaction

By default zerostack only compacts the conversation *between* turns, after a
response finishes, when the accumulated session history exceeds
`context_window - reserve_tokens`. A single long turn (many tool calls and large
tool results) can still blow past the model's real context limit before that
check ever runs, because the in-flight tool traffic never enters the session's
token estimate.

`mid_turn_compact_threshold` opts in to a second, *within-turn* check. On every
provider call zerostack compares the real provider-reported prompt size against
`context_window`; when the ratio crosses the threshold it stops the run at a
clean boundary, compacts, and resumes the same task on the compacted history.

- **Unset by default.** With no value set, behavior is unchanged: no mid-turn
  compaction. Setting a value is the opt-in.
- **Gated by `compact_enabled`.** `compact_enabled` is the master switch. If it
  is `false`, `mid_turn_compact_threshold` is ignored and nothing compacts.
- **Range.** A fraction in `(0.0, 1.0]`. An out-of-range value is ignored
  (mid-turn compaction stays off) and zerostack prints a warning at startup
  explaining the correct form, rather than failing silently. `0.80` is a
  reasonable starting value; it leaves headroom below the
  context window while still keeping the live prompt small enough to avoid the
  attention degradation ("context rot") that large, full context windows suffer.

```toml
compact_enabled = true            # master switch (default true)
context_window = 24576
mid_turn_compact_threshold = 0.80 # compact mid-turn at 80% real prompt pressure
```

## OpenAI API styles and custom headers

The `openai` provider (and any custom provider with `"provider_type": "openai"`)
can talk to either of rig's two OpenAI transports:

- **`responses`** — the Responses API (`/responses`). Default for
  `api.openai.com` (no `base_url`). Required for GPT-5-series models, which
  reject `max_tokens` on Chat Completions and expect `max_completion_tokens`.
- **`completions`** — the Chat Completions API (`/chat/completions`). Default
  when a custom `base_url` is set, because most OpenAI-compatible gateways
  (vLLM, LiteLLM, self-hosted) implement only this endpoint.

Set `api_style` to override the auto-detected default — for example, to force
`completions` against a gateway, or `responses` against an endpoint that
actually implements `/responses`.

Custom providers may also send arbitrary HTTP headers, which is useful for
gateways behind an auth proxy such as Cloudflare Access. Header values support
`${ENV_VAR}` expansion, so secrets stay in the environment rather than in the
config file:

```json
{
  "custom_providers": {
    "company-gateway": {
      "provider_type": "openai",
      "base_url": "https://gateway.example.com/v1",
      "api_key_env": "GATEWAY_API_KEY",
      "headers": {
        "cf-access-client-id": "${CF_ACCESS_CLIENT_ID}",
        "cf-access-client-secret": "${CF_ACCESS_CLIENT_SECRET}"
      }
    }
  }
}
```

The optional `timeout_secs` field overrides the default HTTP timeout for the
provider. TLS certificate verification can be disabled with
`"danger_accept_invalid_certs": true` (for self-signed or internal-CA
gateways) — use with care, as it makes the connection vulnerable to MITM.

## Colors

The `colors` object accepts three optional string fields, each of which can be a
named color or hex color (e.g. `"#1e1e2e"`). Named colors are case-insensitive.
Accepted values:

- `chat_background` — background color for the main conversation buffer.
- `input_background` — background color for the text input area.
- `status_background` — background color for the status bar (lowest line).

Supported named colors: `reset`, `black`, `red`, `green`, `yellow`, `blue`,
`magenta`, `cyan`, `white`, `grey`, `dark_grey`, `dark_red`, `dark_green`,
`dark_yellow`, `dark_blue`, `dark_magenta`, `dark_cyan`.

Example:
```json
{
  "colors": {
    "chat_background": "#1e1e2e",
    "input_background": "#181825",
    "status_background": "#11111b"
  }
}
```

Permission actions are lowercase strings: `allow`, `ask`, or `deny`. Each tool
rule can be a single action or an object mapping patterns to actions. Supported
permission tool keys are `bash`, `read`, `write`, `edit`, `grep`, `find_files`,
`list_dir`, and `write_todo_list`. MCP-backed tools are checked under
`mcp_tool:{server_name}:{tool_name}`. Use `"*"` for the default action,
`external_directory` for absolute-path rules outside the working directory, and
`doom_loop` for repeated identical tool calls (default: `ask`). If `bash` is
omitted, zerostack installs its built-in safe bash allow/deny rules.

There are two config fields for controlling permissions by pattern:

- **`permission`** — patterns are treated as globs (e.g. `**/*.rs`, `src/**`).
- **`permission-regex`** — same structure as `permission`, but patterns are
  treated as regular expressions (e.g. `.*\.rs$`, `^src/`). Regex patterns are
  unanchored — use `^` and `$` to match the full input.

Both fields can be used together; rules from both are merged. If both define a
default action (`"*"`), the glob default takes precedence.

As a TOML-friendly alternative to the nested `permission` object, you can use
`permission-allow`, `permission-ask`, and `permission-deny` at the top level.
Each is a map from tool name to a list of glob patterns. These work side by
side with the `permission` field and are especially convenient in TOML configs:

```toml
permission-allow = { read = ["src/**", "tests/**"] }
permission-ask = { bash = ["rm **"] }
permission-deny = { write = ["/etc/**", "/usr/**"] }
```

In JSON:
```json
{
  "permission-allow": {
    "read": ["src/**", "tests/**"]
  },
  "permission-ask": {
    "bash": ["rm **"]
  },
  "permission-deny": {
    "write": ["/etc/**", "/usr/**"]
  }
}
```

A `permission-regex` example in JSON:

```json
{
  "permission-regex": {
    "*": "ask",
    "read": {
      "\\.md$": "allow",
      "\\.rs$": "ask"
    },
    "bash": {
      "^cargo (test|check|build)$": "allow",
      "^rm ": "deny"
    }
  }
}
```

When compiled with MCP support, `mcp_servers` accepts command-based and URL-based
servers:

```json
{
  "mcp_servers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "env": {}
    },
    "remote-search": {
      "url": "https://example.com/mcp",
      "headers": {
        "authorization": "Bearer token"
      }
    }
  }
}
```

### Recommended MCP servers

When `mcp_servers` is not explicitly set, three recommended MCP servers are
available. Each can be toggled with a boolean config key (all default to the
listed API key environment variable when that variable is set):

| Key                    | Default | Description                                     | Env var              |
| ---------------------- | ------- | ----------------------------------------------- | -------------------- |
| `enable-exa-mcp`       | `true`  | Exa web search (mcp.exa.ai)                     | `EXA_API_KEY`        |
| `enable-context7-mcp`  | `false` | Context7 documentation lookup (mcp.context7.com) | `CONTEXT7_API_KEY`   |
| `enable-grepapp-mcp`   | `false` | Grep.app semantic code search (mcp.grep.app)     | `GREP_APP_API_KEY`   |

Set `enable-exa-mcp = false` to disable the Exa default without touching
`mcp_servers`. Set `"mcp_servers": {}` to disable all MCP auto-configuration.

## ACP (Agent Communication Protocol) configuration

When compiled with the `acp` feature, zerostack can act as an ACP agent server.
The following config keys are available:

| Key           | Type    | Description                                            |
| ------------- | ------- | ------------------------------------------------------ |
| `acp_servers` | object  | Named ACP server configurations (see below)            |
| `acp_host`    | string  | TCP bind host for ACP server (default: stdio mode)     |
| `acp_port`    | integer | TCP bind port for ACP server (default: 7243)           |

ACP server configs (in `acp_servers`) support two transport types:

```json
{
  "acp_servers": {
    "tcp-server": {
      "host": "127.0.0.1",
      "port": 7243,
      "api_key": "optional-key"
    }
  }
}
```

When `--acp` is passed without `--acp-host`, zerostack runs in stdio mode
(the editor spawns it as a subprocess). With `--acp-host`, it listens on TCP.

## TOML configuration

zerostack prefers `config.toml` over `config.json` when both exist. If neither
file exists, a default `config.toml` is created automatically.

TOML is especially well suited for zerostack's permission rules and structured
settings. Hyphenated keys such as `permission-regex`, `permission-allow`,
`permission-ask`, and `permission-deny` are idiomatic in TOML and avoid deeply
nested tables:

```toml
permission-allow = { read = ["src/**", "tests/**"] }
permission-ask = { bash = ["rm **"] }
permission-deny = { write = ["/etc/**", "/usr/**"] }
```

For more complex configurations, explicit TOML tables provide clear structure:

```toml
[permission]
"*" = "ask"

[permission.bash]
"cargo test" = "allow"
"rm **" = "deny"

[permission.write]
"**/*.rs" = "allow"
"**" = "ask"
```

### Key naming in TOML

All top-level keys use kebab-case when they contain hyphens (e.g.
`permission-allow`, `allow-all-mcp-calls`). Simple keys use the same name as
their JSON counterpart. Quoted keys (`"*"`, `"**"`) are required when the key
contains special characters like `*` or `/`.

## Edit System Modes

zerostack supports two edit systems, selectable via `edit_system` config key,
`--edit-system` CLI flag, or `/editsys` slash command:

### `similarity` (default)

The classic aider-style SEARCH/REPLACE format. The LLM copies exact text from
read output into `<<<<<<< SEARCH` blocks and provides replacements in
`>>>>>>> REPLACE` blocks. Falls back to whitespace normalization and fuzzy
matching when the exact text doesn't match.

```
edit_system = "similarity"
```

### `hashedit`

Tag-based edits using CRC-32 line hashes and file-level CAS (check-and-set)
tokens. The read tool annotates each line with an 8-char hex CRC-32 tag (e.g.
`"  10|f1e2d3c4 int count = 10;"`) and a file-level CRC header. The edit tool
receives tagged lines from the read output and provides only the replacement
text — no old-text reproduction needed.

Key advantages:
- **Token-efficient**: No old-text reproduction (significant savings for
  deletions and large edits)
- **CAS-guarded**: File-level CRC prevents applying edits to stale content
- **Reliable**: Per-line tag validation catches content mismatches

```
edit_system = "hashedit"
```

Switching between modes is immediate and does not require agent restart.
The `/editsys` `similarity` and `/editsys` `hashedit` slash commands
provide the same functionality at runtime.

## Prompt directives

Custom prompt `.md` files may include a `%%mode=<mode>` directive on the
**first line** to automatically switch the security mode when the prompt
is activated (via `/prompt <name>` or as the `default_prompt`).

Valid modes: `standard`, `restrictive`, `readonly`, `guarded`, `yolo`.

Use `%%mode=last_user_mode` to keep (or restore) the mode the user last
set explicitly via `/mode` or startup config — useful when a prompt wants
to avoid overriding the user's chosen mode.

The directive line is stripped from the prompt content before it reaches
the agent.

Example `ask.md`:

```markdown
%%mode=readonly

## Read-Only Mode

You are in read-only mode. Only read files and explore.
```

Example `code.md` that defers to the user's mode:

```markdown
%%mode=last_user_mode

## Coding Mode

Write well-tested code. Follow project conventions.
```

The mode change is applied when the prompt is activated and persists
until changed again by `/mode`, another prompt directive, or a restart.
The status bar shows `| mode:<name>` when the mode is not `standard`.

## Chain-of-Prompts

When enabled, after the agent finishes responding with a `brainstorm`, `plan`,
or `code` prompt, the status bar shows `Continue to <next>? [Yes/But/No]`.
The user's next input is interpreted as a chain decision:

- **Yes** (`y`/`yes`) — switch to the next prompt and auto-submit a transition message.
- **But** (`but <msg>` / `b <msg>` / `yes but <msg>`) — same as yes, but prepend
  `<msg>` as an additional instruction to the transition message.
- **No** (`n`/`no`) — decline the chain, continue normally.

Typing anything that doesn't match these patterns clears the chain and
processes the input as a normal message.

### Phases

| Transition | Default | Description |
|-----------|---------|-------------|
| `brainstorm-to-plan` | `true` | After brainstorming, prompt to move to planning |
| `plan-to-code` | `true` | After planning, prompt to start coding |
| `code-to-review` | `false` | After coding, prompt to run a review |

### TOML

```toml
[chain]
brainstorm-to-plan = true
plan-to-code = true
code-to-review = false
```

### JSON

```json
{
  "chain": {
    "brainstorm-to-plan": true,
    "plan-to-code": true,
    "code-to-review": false
  }
}
```

## Advisor

The advisor tool lets the agent consult a stronger reviewer model (or the
user, in human-handoff mode) for strategic guidance before making important
decisions. This follows the [advisor strategy](https://claude.com/blog/the-advisor-strategy):
a cheaper "executor" model drives the task and escalates to a more capable
model only when needed.

### TOML

```toml
[advisor]
enabled = true
model = "deepseek/deepseek-v4-pro"
# provider = "openrouter"         # defaults to main provider
# max_uses = 3                    # max advisor calls per request (nil = unlimited)
# human_handoff = false           # route advisor calls to the user instead
# advisor_kilobytes_limit = 256   # max KB of conversation context (split half head / half tail)
```

### JSON

```json
{
  "advisor": {
    "enabled": true,
    "model": "deepseek/deepseek-v4-pro",
    "max_uses": 3,
    "human_handoff": false,
    "advisor_kilobytes_limit": 256
  }
}
```

### CLI flags

| Flag | Description |
|------|-------------|
| `--advisor` | Enable the advisor tool |
| `--advisor-model <name>` | Advisor model name |
| `--advisor-provider <name>` | Provider for the advisor model |
| `--advisor-max-uses <n>` | Max advisor calls per request |
| `--advisor-human-handoff` | Route advisor calls to the user |
| `--advisor-kilobytes-limit <n>` | Max KB of conversation context sent to advisor (default: 256) |

### Human handoff mode

When `human_handoff = true`, the agent's advisor calls are redirected to the
user instead of a second model. The agent pauses, shows its question, and the
user types a response. This is useful for:

- Reviewing the agent's approach before it writes code
- Stepping in when the agent is stuck or uncertain
- Teaching the agent your preferences interactively

### Runtime control

The `/advisor` slash command provides runtime control:

```
/advisor                    Show current advisor status
/advisor on|off             Enable or disable the advisor
/advisor handoff [on|off]   Toggle human handoff mode
/advisor model <name>       Change the advisor model
/advisor max-uses <n>       Set max advisor calls per request (0 = unlimited)
/advisor context-limit <n>  Set max kilobytes of conversation context
```
