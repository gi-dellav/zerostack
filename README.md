# zerostack

Minimal coding agent written in Rust, inspired by [pi](https://pi.dev/docs/latest/usage) and [opencode](https://opencode.ai/).

## Features

- **Multi-provider**: OpenRouter, OpenAI, Anthropic, Gemini, Ollama, plus custom providers
- **Standard tools**: all of the standard tools exposed to coding agents, as described by the opencode documentation.
- **Permission system**: four configurable modes with per-tool patterns, session allowlists, and external directory policies
- **Session management**: save/load/resume sessions, auto-compaction to stay within context windows
- **Terminal UI**: crossterm-based, markdown rendering, mouse selection/copy, scrollback, reasoning visibility toggle
- **Prompts system**: switch between system prompt modes at runtime (`code`, `plan`, `review`, `debug`, etc.) to tailor the agent's behavior to the task without having to manage Skills.
- **MCP support**: connect MCP servers for extended tooling (exposed as an optional compile-time feature)
- **Integrated Exa search**: allows for WebFetch and WebSearch tools
- **Integrated Ralph Wiggum loops**: looping capabilities for long-horizon tasks
- **Integrated Git Worktrees integration**: Use `/worktree` to move the agent from one worktree to another.

**NOTE**: Windows support is not tested is any way, but feel free to try and open an issue if you encounter any bugs!

## Performance

*zerostack* is one of the smallest and most performant coding agents on the market.

- Lines of code: ~7k LoC
- Binary size: 8.9MB
- RAM footprint: ~8MB on an empty session, ~12MB when working (vs ~300MB for opencode or other JS-based coding agents)
- CPU usage: 0.0% on idle, ~1.5% when using tools (measured on an Intel i5 7th gen, vs ~2% on idle and ~20% when working for opencode)

## Installation

In order to install *zerostack*, you must have Cargo and git installed. Then, run:

```bash
cargo install zerostack
```

You are now ready to work with a lightweight coding agent! (Note: you can also find pre-built binaries on Github Releases)

### Optional: sandbox mode

Install [bubblewrap](https://github.com/containers/bubblewrap) for `--sandbox`,
which runs every bash command inside an isolated environment to protect your
system from accidental or malicious damage:

```bash
# Debian/Ubuntu
apt install bubblewrap

# Fedora
dnf install bubblewrap

# Arch
pacman -S bubblewrap
```

## Quick start

```bash
# Set your API key (OpenRouter is default)
export OPENROUTER_API_KEY="[api_key]"

# Interactive session (default prompt: code)
zerostack

# One-shot mode
zerostack -p "Explain this project"

# Continue last session
zerostack -c

# Explicit provider/model
zerostack --provider openrouter --model deepseek/deepseek-v4-flash
```

## Prompts system

*zerostack* includes a set of built-in system prompts that change the agent's behavior and tone.  
The idea is to build a complete suite of prompts that can fully substitute skills like [superpower](https://github.com/obra/superpowers) or the [Claude's official skills](https://github.com/anthropics/claude-plugins-official/tree/main).  
You can switch between different prompts or list all registered prompts using `/prompt`.  

Built-in prompts:

| Prompt | Description |
|---|---|
| **`code`** (default) | Coding mode with full file and bash tool access, TDD workflow |
| **`plan`** | Planning-only mode — explores and produces a plan without writing code |
| **`review`** | Code review mode — reviews for correctness, design, testing, and impact |
| **`debug`** | Debug mode — finds root cause before proposing fixes |
| **`ask`** | Read-only mode — only read/grep/glob permitted, no writes or bash |
| **`brainstorm`** | Design-only mode — explores ideas and presents designs without code |
| **`frontend-design`** | Frontend design mode — distinctive, production-grade UI |
| **`review-security`** | Security review mode — finds exploitable vulnerabilities |
| **`simplify`** | Code simplification mode — refines for clarity without changing behavior |
| **`write-prompt`** | Prompt writing mode — creates and optimizes agent prompts |

You can also create custom prompts by placing markdown files in
`$XDG_CONFIG_HOME/zerostack/prompts/` and referencing them by name.

Additionally, the agent automatically loads `AGENTS.md` or `CLAUDE.md` from the
project root or any ancestor directory, injecting their contents into the
system prompt. Use `-n` / `--no-context-files` to disable this.

## Permission system

zerostack has four permission modes, from safest to most permissive:

1. **restrictive** (`-R`): every tool action prompts for approval unless
   explicitly allowed in config
2. **standard** (default): safe commands (ls, cd, git log, cargo check) are
   auto-approved; writes and destructive operations ask
3. **accept-all** (`--accept-all`): auto-approves all operations inside the
   working directory; external paths prompt for confirmation
4. **yolo** (`--yolo`): auto-approves everything without prompting

Permissions can be configured per-tool with granular glob patterns in the
config file. For example, you can allow `write **.rs` automatically while
always asking before writing to other files.

A **session allowlist** persists approved decisions for the duration of the
session, so you don't have to repeatedly confirm the same operation.

**Doom-loop detection**: identical tool calls repeated 3+ times trigger a
warning prompt (or denial depending on your config), preventing runaway agents
from spamming destructive operations.

## Slash commands

This is a list of the most important slash commands:

- `/model` — Switch model
- `/thinking` — Set thinking level
- `/clear` — Clear conversation
- `/session` — List/save/load sessions
- `/loop` — Schedule recurring prompts
- `/prompt` — List or change the agent's prompt
- `/mode` — Set the permission system's mode

To see all of the commands, use `/help`.

## Session management

Sessions are saved to `$XDG_DATA_HOME/zerostack/sessions/`. Use `-c` to
resume the most recent session, `-r` to browse and select one, or
`--session <id>` to load a specific session.

## Loop system

*zerostack* includes an iterative coding loop for long-horizon tasks. The agent repeatedly reads the task, picks an item from the plan, works on it, runs tests, updates the plan, and loops until the task is complete or the iteration limit is reached.

**NOTE** The loop system is an *experimental* feature.

### Loop usage

```
/loop Implement the user authentication system
/loop stop
/loop status
```

- `/loop <prompt>` — Start a loop with the given prompt
- `/loop stop` — Stop the active loop
- `/loop status` — Show current loop state

Each iteration includes the original task, the evolving `LOOP_PLAN.md`, a summary of the previous iteration, and any validation output. Non-slash input is blocked while a loop is active.

### Headless loops via CLI

```
zerostack --loop --loop-prompt "Refactor the API" --loop-max 10 --loop-run "cargo test"
```

| Flag | Description |
|---|---|
| `--loop` | Enable headless loop mode |
| `--loop-prompt <text>` | Prompt for each iteration |
| `--loop-plan <path>` | Custom plan file path (default: `LOOP_PLAN.md`) |
| `--loop-max <N>` | Maximum iterations (default: unlimited) |
| `--loop-run <cmd>` | Validation command to run after each iteration |

## Git worktrees integration

*zerostack* provides a branch-per-task workflow using git worktrees. You can create, work in, merge, and exit worktrees entirely from the chat UI.

**NOTE** The git worktrees integration is an *experimental* feature.

### Git worktree usage

The worktrees integrations offers 3 slash commands:

| Command | Description |
|---|---|
| `/worktree <name>` | Create a git worktree on branch `<name>` and move into it (skips creating it if it already exists) |
| `/wt-merge [branch]` | Merge the worktree branch into `[branch]` (default: `main`/`master`), push, clean up, and return to the main repo |
| `/wt-exit` | Return to the main repo without merging |

### Example workflow for git worktrees

1. **Create** — `/worktree feature-x` creates a new branch and worktree directory and moves you there.
2. **Work** — Use zerostack normally; changes stay on the feature branch.
3. **Merge** — `/wt-merge` tells the agent to merge the branch, push, clean up, and return to the main repo.
4. **Exit** — `/wt-exit` immediately returns to the main repo without merging.

## Supported providers

- OpenRouter (default)
- OpenAI-compatible (vLLM, LiteLLM, etc.)
- Anthropic
- Gemini
- Ollama
- Zima Labs

Custom providers can be configured with any base URL and API key environment
variable in `$XDG_CONFIG_HOME/zerostack/config.json`.

## License

GPL-3.0-only
