# Architecture — zerostack v1.6.2

Minimal coding agent in Rust, optimized for memory footprint and performance.
Single crate, no workspace. All source under `src/`.

## Directory Layout

| Path | Responsibility |
|---|---|
| `src/main.rs` | Entry point, CLI dispatch, mode routing |
| `src/cli.rs` | `clap::Parser` CLI argument definition |
| `src/provider.rs` | LLM provider abstraction (type-erased: `AnyClient`, `AnyModel`, `AnyAgent` enums) |
| `src/auth.rs` | API key resolution (`AuthResolver`, `ProviderKind` enum) |
| `src/event.rs` | `AgentEvent` (streaming LLM output) and `UserEvent` (TUI input) enums |
| `src/agent/` | Agent lifecycle: `builder.rs` (rig Agent construction + tool injection), `runner.rs` (spawn, stream), `prompt.rs` (system prompts), `tools/` (8 core tool impls: read, write, edit, bash, grep, find_files, list_dir, todo; plus feature-gated `TaskTool` and `AdvisorTool`) |
| `src/session/` | Session state: `mod.rs` (messages, compactions, costs), `storage.rs` (JSON file I/O), `chat_history.rs` |
| `src/permission/` | Security: `checker.rs` (glob+regex rules, doom-loop detection), `ask.rs` (user prompt UI), `pattern.rs` |
| `src/ui/` | Custom TUI on crossterm (no ratatui): `mod.rs` (event loop), `terminal.rs` (raw mode guard), `renderer.rs` (line buffer + viewport), `input/` (text editor + pickers), `status.rs`, `markdown.rs`, `event_handler.rs`, `cmd_picker.rs` |
| `src/context/` | Context gathering: embedded prompt themes (`prompts.rs`, `themes.rs`), AGENTS.md/ARCHITECTURE.md loading |
| `src/config/` | Configuration: `load.rs` (TOML/YAML/JSON from disk+env), `types.rs` (QuickModel, CustomProvider, Colors, EditSystem) |
| `src/extras/` | Feature-gated extensions: `loop/` (headless), `mcp/` (MCP client), `acp/` (ACP server), `memory/` (persistent memory), `subagents/` (parallel task delegation), `git_worktree/`, `archmd/`, `advisor/` (external model consultation, `/advisor`), `multimodal/` (image/PDF ingestion, gated separately by `multimodal`/`pdf`), `hooks/` (lifecycle hook dispatch: `PreToolUse`/`PostToolUse`/`Stop`/`UserPromptSubmit`/session+subagent events, trust-hash confirmation). Two subsystems are always compiled (not feature-gated): `chain/` (brainstorm→plan→code prompt transitions) and `status_signals` (Unix-socket start/stop/git-conflict signals, itself gated by the `status-signals` feature at the call site) |
| `src/sandbox.rs` | `bwrap`/`zerobox` command wrapping |
| `src/fs.rs` | Filesystem utilities |
| `src/pricing.rs` | Token pricing constants |

## Key Types & Relationships

- **`Config`** (`src/config/mod.rs:23`) — central deserialized config, drives all runtime behavior.
- **`Cli`** (`src/cli.rs:9`) — `clap::Parser` args, overrides `Config` fields.
- **`AnyClient`** (`src/provider.rs:153`) / **`AnyModel`** (`:545`) / **`AnyAgent`** (`:562`) — type-erased enums wrapping rig's provider-specific clients (OpenAI, Anthropic, Gemini, Ollama, OpenRouter). `AnyAgent` provides `run_print()` and `spawn_runner()`. No custom traits — enum dispatch replaces dynamic dispatch.
- **`AgentRunner`** (`src/agent/runner.rs:17`) — holds `mpsc::Receiver<AgentEvent>`, spawned via `spawn_agent()`.
- **`AgentEvent`** (`src/event.rs:4`) — `Token`, `Reasoning`, `ToolCall`, `ToolResult`, `SubagentToolCall`, `Error`, `Done`.
- **`UserEvent`** (`src/event.rs:64`) — `Key`, `ScrollUp/Down`, `Resize`, `Paste`, `MouseDown/Drag/Up`.
- **`Session`** (`src/session/mod.rs:61`) — serializable state: messages, compactions, costs, permission allowlist, model/provider info.
- **`PermissionChecker`** (`src/permission/checker.rs:29`) — dual-layer (glob + regex) rules, doom-loop detection, `SecurityMode` dispatch.
- **`TerminalGuard`** (`src/ui/terminal.rs:10`) — RAII for raw mode, alt screen, mouse capture.
- **`Renderer`** (`src/ui/renderer.rs:52`) — line-buffered viewport, markdown rendering, scroll/selection.
- **`InputEditor`** (`src/ui/input/mod.rs:21`) — text buffer, cursor, history, kill-ring, picker integration.
- **`ContextFiles`** (`src/context/mod.rs:57`) — loaded agents, prompts, themes, architecture docs.
- **`HookDispatcher`** (`src/extras/hooks/dispatcher.rs:60`, feature `hooks`) — merges `PreToolUse` verdicts (`Allow`/`Defer`/`Ask`/`Deny`, most severe wins) and applies `PostToolUse`/lifecycle `Decision`s (`Continue`/`Block`/`Rewrite`). Wraps every tool via `wrap_from_global()` (`src/agent/builder.rs:276`), outside each tool's own `PermissionChecker` check.

## Control Flow

```
CLI parse (main.rs:150) → config load → context load → session load
  │
  ├── --print-config → print and exit
  ├── --acp → extras::acp::serve()
  ├── --print → single agent.run_print() response
  ├── --loop → run_headless_loop() iterative mode
  └── (default) → ui::run_interactive()
```

### Interactive TUI Event Loop (`src/ui/mod.rs`)

Single `tokio::select!` with 6 branches plus an `else` fallback (line 1119):
1. **`UserEvent` from `user_rx`** — keyboard/mouse/resize/paste from background event thread (polls crossterm every 50ms idle, 10ms while coalescing a paste burst)
2. **Background agent prebuild** from `prebuild_rx` — consumed once idle, so MCP connection notices land in the transcript instead of racing the alt-screen TUI
3. **`AgentEvent` from `agent_rx`** — streaming LLM tokens, tool calls, errors
4. **Permission `AskRequest` from `ask_rx`** — user must approve/reject tool calls
5. **`BtwEvent` from `btw_rx`** — parallel `/btw` side-question results, rendered but never written to the session
6. **Periodic refresh** (100ms, gated on `is_running`) — spinner animation
7. **`else`** — polls the prebuild receiver and sleeps 50ms when nothing else is ready

Key dispatch: `InputEditor::handle_key()` → `Some(text)` triggers `spawn_agent()` → stream events via `handle_agent_event()` which writes to `Renderer` and appends to `Session`.

## Data Flow

```
User input → InputEditor (buffer) → spawn_agent(prompt + history)
  │
  ▼
Agent (rig) → CompletionModel (LLM API)
  │
  ▼ streaming
AgentEvent stream (Token, ToolCall, ToolResult, ...)
  │
  ├── handle_agent_event() → Renderer (viewport buffer) → crossterm draw commands
  ├── ToolCall → [feature `hooks`] HookDispatcher PreToolUse → {Allow, Ask, Deny, rewritten input}
  │     └── tool.call() → PermissionChecker.check() → {Allowed, Ask, Denied}
  │           ├── Ask → permission_handler (user approves/rejects via UI)
  │           └── Allowed → tool execution (bash/read/write/edit/grep/etc.) → [feature `hooks`] PostToolUse rewrite
  └── Done → Session.append() → session::storage::save_session()
```

Session is serialized to JSON files in `$XDG_DATA_HOME/zerostack/sessions/`. Chat history appended to `$XDG_DATA_HOME/zerostack/chat_history.jsonl`.

## Design Decisions

1. **Custom TUI over crossterm (no ratatui)** — keeps binary size minimal; project has its own line buffer, markdown renderer, scroll/selection. No widget tree overhead.
2. **Type-erased enums, not trait objects** — `AnyAgent` enum wraps each provider variant. Avoids `dyn CompletionModel` lifetime issues; matching on enum is faster than vtable dispatch. (`src/provider.rs:153`, `:545`, `:562`)
3. **Permission: dual-layer (glob + regex) rules** — glob for fast path, regex for complex patterns. Doom-loop detection tracks repeated identical tool calls. (`src/permission/checker.rs:29`)
4. **Session compaction** — when token count approaches context window, old messages are summarized and dropped, preserving a summary prefix. (`Compaction` struct at `src/session/mod.rs:46`, gate at `:528`)
5. **Feature-gated extras** — `loop`, `git-worktree`, `mcp`, `subagents`, `archmd`, `status-signals`, `multithread` are the default features; `acp`, `memory`, `advisor`, `hooks`, `multimodal`, `pdf` are opt-in. Extras don't bloat the core binary.
6. **Single-threaded tokio by default** — `#[tokio::main(flavor = "current_thread")]` unless `multithread` feature enabled. Keeps resource usage low for a CLI tool.
7. **Hooks wrap tools, they don't replace permission checks** — `HookDispatcher` (feature `hooks`) decorates every tool at construction time (`wrap_from_global()`); it runs strictly outside each tool's own `PermissionChecker` call, so a `PreToolUse` rewrite can only narrow a request, never grant a permission the checker would otherwise deny. (`src/extras/hooks/dispatcher.rs`)

## Dependencies

| Crate | Use |
|---|---|
| `rig 0.39` | Agent framework: prompt hooks, tool system, streaming, provider clients (OpenAI, Anthropic, Gemini, Ollama, OpenRouter) |
| `clap 4` | Derive-based CLI argument parsing (`src/cli.rs:9`) |
| `crossterm 0.29` | Terminal raw mode, color, cursor, mouse, paste events — TUI foundation |
| `tokio 1` | Async runtime (current_thread default), channels (`mpsc`), process, fs |
| `serde + serde_json + serde_yaml_ng + toml` | Config (TOML/YAML/JSON), session serialization (JSON) |
| `chrono`, `uuid` | Session timestamps and IDs |
| `pulldown-cmark 0.13` | Markdown → styled lines for TUI rendering |
| `ignore 0.4` | `.gitignore`-aware file traversal (`find_files` tool) |
| `regex 1` | Permission pattern matching |
| `reqwest 0.13` | HTTP client (provider API calls via rig) |
| `tracing + tracing-subscriber` | Structured logging (`RUST_LOG` env var) |
| `mimalloc` | Global allocator (size + speed) |
| `compact_str`, `smallvec` | Heap-efficient small-string/small-vector types |

Optional (`mcp` feature): `rmcp 2.0` (MCP client with child-process + HTTP transport). Optional (`acp` feature): `agent-client-protocol 1.0.1`, `blocking`. Optional (`multimodal` feature): `rig/image`; `pdf` feature adds `rig/pdf` (implies `multimodal`). `advisor` and `hooks` are pure-logic features, no extra dependencies.

## Entry Points

- **`main()`** (`src/main.rs:149`) — all modes dispatch from here
- **`--print`** / `-p` — `agent.run_print()` → single reply, then exit (`main.rs:774`)
- **`--loop`** — `run_headless_loop()` → iterative prompt/validate loop (branch at `main.rs:891`, definition at `:1160`)
- **`--acp`** — `extras::acp::serve()` → ACP server mode (`main.rs:417`)
- **Default (no flags)** — `ui::run_interactive()` → full TUI (call at `main.rs:931`, definition at `src/ui/mod.rs:751`)
- **`--resume`** / `--continue` / `--session <id>` — loads prior session before entering TUI/print
- **`--advisor`** (feature `advisor`) — registers `AdvisorTool` so the agent can consult a second model for strategic guidance mid-session (`src/agent/builder.rs:269-273`)
- **`--no-hooks`** / **`--hooks-test`** (feature `hooks`) — disables the hook dispatcher, or dry-runs a single hook invocation against stdin and exits (`src/main.rs:178`, `:191`)
- **`--status-socket`** (feature `status-signals`) — emits `start`/`stop`/`git-conflict` events over a Unix socket for external status bars (`src/cli.rs:260`)
