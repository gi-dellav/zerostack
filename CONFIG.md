# Configuration

zerostack reads an optional JSON config file named `config.json` from its config
folder:

- If `ZS_CONFIG_DIR` is set: `$ZS_CONFIG_DIR/config.json`
- Otherwise: the platform config directory joined with `zerostack/config.json`
  (for example `$XDG_CONFIG_HOME/zerostack/config.json` on Linux)
- Fallback: `$HOME/.config/zerostack/config.json`

All config keys are optional. CLI flags and their environment-backed values
(such as `ZS_PROVIDER` and `ZS_MODEL`) take precedence where both exist.

Example:

```json
{
  "provider": "openrouter",
  "model": "deepseek/deepseek-v4-flash",
  "max_tokens": 8192,
  "temperature": 0.7,
  "context_window": 128000,
  "reserve_tokens": 16384,
  "keep_recent_tokens": 20000,
  "compact_enabled": true,
  "default_prompt": "code",
  "default_permission_mode": "standard",
  "show_tool_details": false,
  "custom_providers": {
    "local-vllm": {
      "provider_type": "openai",
      "base_url": "http://localhost:8000/v1",
      "api_key_env": "VLLM_API_KEY"
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

Accepted top-level keys:

| Key                       | Type    | Description                                                                                                                                                                 |
| ------------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `provider`                | string  | Provider name. Built-ins are `openrouter`, `openai`, `anthropic`, `gemini`/`google`, and `ollama`; custom provider aliases are also accepted. Default: `openrouter`.        |
| `model`                   | string  | Model name. Default: `deepseek/deepseek-v4-flash`.                                                                                                                          |
| `max_tokens`              | integer | Maximum response tokens. Default: `8192`.                                                                                                                                   |
| `max_agent_turns`         | integer | Maximum agent turns per response. Default: `100`.                                                                                                                           |
| `temperature`             | number  | Model temperature value. Only configurable via the `--temperature` CLI flag (`0.0` to `2.0`). Config-file value is parsed but not currently applied.                        |
| `no_tools`                | boolean | Disable all tools. Default: `false`.                                                                                                                                        |
| `no_context_files`        | boolean | Disable loading global/project `AGENTS.md` and `CLAUDE.md` context files. Default: `false`.                                                                                 |
| `context_window`          | integer | Session context-window size used for status and auto-compaction. Default: `128000`.                                                                                         |
| `reserve_tokens`          | integer | Tokens to reserve before compaction is triggered. Default: `16384`.                                                                                                         |
| `keep_recent_tokens`      | integer | Approximate recent-token budget kept verbatim during compaction. Default: `20000`.                                                                                          |
| `compact_enabled`         | boolean | Enable automatic conversation compaction. Default: `true`.                                                                                                                  |
| `custom_providers`        | object  | Map of provider aliases to `{ "provider_type", "base_url", "api_key_env" }`. `provider_type` must resolve to one of the built-in provider types; `api_key_env` is optional. |
| `permission`              | object  | Permission rules; see the permission config notes below.                                                                                                                    |
| `restrictive`             | boolean | Select restrictive permission mode. Overridden by `accept_all`/`yolo` if those are also true.                                                                               |
| `accept_all`              | boolean | Select accept mode, equivalent to `--accept-all`. Overridden by `yolo` if true.                                                                                             |
| `yolo`                    | boolean | Select yolo mode, auto-approving all operations.                                                                                                                            |
| `sandbox`                 | boolean | Run bash commands in the bubblewrap sandbox. Default: `false`.                                                                                                              |
| `default_permission_mode` | string  | Permission mode when no mode boolean/CLI flag is set. Use `standard`, `restrictive`, `accept`, or `yolo`.                                                                   |
| `show_tool_details`       | boolean | Show tool-result previews in the TUI. Default: `false`.                                                                                                                     |
| `default_prompt`          | string  | Prompt name to activate on startup. Default: `code`.                                                                                                                        |
| `mcp_servers`             | object  | MCP server map when compiled with the `mcp` feature. When omitted, defaults to a single Exa Web Search server; see below.                                                   |
| `acp_servers`             | object  | ACP server config map when compiled with the `acp` feature. See the ACP section below.                                                                                       |
| `acp_host`                | string  | TCP bind host for ACP server mode (equivalent to `--acp-host`).                                                                                                              |
| `acp_port`                | integer | TCP bind port for ACP server mode (equivalent to `--acp-port`, default: 7243).                                                                                               |

Permission actions are lowercase strings: `allow`, `ask`, or `deny`. Each tool
rule can be a single action or an object mapping glob-like patterns to actions.
Supported permission tool keys are `bash`, `read`, `write`, `edit`, `grep`,
`find_files`, `list_dir`, and `write_todo_list`. MCP-backed tools are
checked under `mcp_tool:{server_name}:{tool_name}`. Use `"*"` for the
default action, `external_directory` for absolute-path rules outside the
working directory, and `doom_loop` for repeated identical tool calls
(default: `ask`). If `bash` is omitted, zerostack installs its built-in
safe bash allow/deny rules.

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
