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
  "reserve_tokens": 16384,
  "keep_recent_tokens": 10000,
  "compact_enabled": true,
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
reserve_tokens = 16384
keep_recent_tokens = 10000
compact_enabled = true
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
| `context_window`          | integer | Session context-window size used for status and auto-compaction. Default: `128000`.                                                                                         |
| `reserve_tokens`          | integer | Tokens to reserve before compaction is triggered. Default: `16384`.                                                                                                         |
| `keep_recent_tokens`      | integer | Approximate recent-token budget kept verbatim during compaction. Default: `10000`.                                                                                          |
| `max_text_file_size`      | integer | Maximum allowed file size in bytes for read/write tool operations. Default: `1048576` (1 MB).                                                                               |
| `deny_repeated_reads`     | boolean | Block repeated reads of the same file section within a session until the file is edited or written. Default: `true`. Set to `false` to allow re-reading.                     |
| `compact_enabled`         | boolean | Enable automatic conversation compaction. Default: `true`.                                                                                                                  |
| `always_show_welcome`     | boolean | Always show the welcome banner on startup, bypassing the one-shot marker file. Default: `false`.                                                                               |
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
| `quick_models`            | object  | Map of quick-model names to `{ "provider", "model" }`. Can be switched with `/models <name>` or `--quick-model=<name>`.                                                      |
| `mcp_servers`             | object  | MCP server map when compiled with the `mcp` feature. When omitted, defaults to a single Exa Web Search server; see below.                                                   |
| `allow_all_mcp_calls`     | boolean | When `true`, permission checks are skipped for all MCP tool calls. Default: `false`.                                                                                        |
| `acp_servers`             | object  | ACP server config map when compiled with the `acp` feature. See the ACP section below.                                                                                       |
| `acp_host`                | string  | TCP bind host for ACP server mode (equivalent to `--acp-host`).                                                                                                              |
| `acp_port`                | integer | TCP bind port for ACP server mode (equivalent to `--acp-port`, default: 7243).                                                                                               |
| `colors`                  | object  | Background color overrides for the TUI. See the colors section below.                                                                                                       |

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

If `mcp_servers` is omitted (`null`) and the `mcp` feature is enabled, zerostack
adds a default Exa Web Search MCP server at `https://mcp.exa.ai/mcp` with the
`x-api-key` header set to `EXA_API_KEY` when that environment variable is set.
Set `"mcp_servers": {}` to disable all MCP servers.

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
