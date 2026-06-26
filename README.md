![banner](https://github.com/gi-dellav/zerostack/blob/main/assets/banner.png?raw=true)

---

# zerostack
Minimal coding agent written in Rust, inspired by [pi](https://pi.dev/docs/latest/usage) and [opencode](https://opencode.ai/).

*blogposts:* [what we built in 2 weeks](https://rocketup.pages.dev/posts/what_we_built_in_2_weeks/) [memory design](https://rocketup.pages.dev/posts/how-zerostack-memory-works/) [subagents design](https://rocketup.pages.dev/posts/how-zerostack-subagents-work/) [xavier's memory analysis](https://xavierforge.dev/en/posts/zerostack-memory-design/)

<a href="https://www.producthunt.com/products/zerostack-coding-agent/reviews/new?utm_source=badge-product_review&utm_medium=badge&utm_source=badge-zerostack&#0045;coding&#0045;agent" target="_blank"><img src="https://api.producthunt.com/widgets/embed-image/v1/product_review.svg?product_id=1236867&theme=light" alt="Zerostack&#0032;Coding&#0032;Agent - A&#0032;minimal&#0032;coding&#0032;agent&#0044;&#0032;with&#0032;a&#0032;bundle&#0032;of&#0032;innovative&#0032;features | Product Hunt" style="width: 250px; height: 54px;" width="250" height="54" /></a>

*note:* Want to support? Consider [donating here](https://ko-fi.com/gidellav); if you are a company interested in sponsoring zerostack, [contact me here](mailto:giuseppe.dellavedova8+sponsor@gmail.com). If you want to support without paying, check out [multistack](https://github.com/gi-dellav/multistack).

## Features

- **Multi-provider**: OpenRouter, OpenAI, Anthropic, Gemini, Ollama, plus custom providers
- **Standard tools**: all of the standard tools exposed to coding agents, as described by the opencode documentation.
- **Permission system**: five configurable modes with per-tool patterns, session allowlists, and configurable mode-to-rule application policies
- **Session management**: save/load/resume sessions, auto-compaction to stay within context windows
- **Terminal UI**: crossterm-based, markdown rendering, mouse selection/copy, scrollback, reasoning visibility toggle
- **Prompts system**: switch between system prompt modes at runtime (`code`, `plan`, `review`, `debug`, etc.) to tailor the agent's behavior to the task without having to manage Skills.
- **MCP support**: connect MCP servers for extended tooling (exposed as an optional compile-time feature)
- **Integrated Exa search**: allows for WebFetch and WebSearch tools
- **Integrated Ralph Wiggum loops**: looping capabilities for long-horizon tasks
- **Integrated Git Worktrees integration**: Use `/worktree` to move the agent from one worktree to another.
- **ACP support** (gated): Agent Communication Protocol server — lets editors (Zed, etc.) connect to zerostack as an ACP agent
- **Persistent memory** (gated): plain-Markdown memory across sessions: a global MEMORY.md plus per-project daily logs, scratchpad, and notes, injected into the system prompt each session
- **Subagents**: Parallel and fast, used for exploring the codebase
- **ARCHITECTURE.md**: Our own companion file for AGENTS.md, it allows to offer a shared core knowledge for all agents working on the same codebase

**NOTE**: Windows support is not tested is any way, but feel free to try and open an issue if you encounter any bugs!

## Performance

_zerostack_ is one of the smallest and most performant coding agents on the market.

- Lines of code: ~17k LoC
- Binary size: 26MB
- RAM footprint: ~16MB on average, with peaks at ~24MB (vs ~300MB with peaks at ~700MB for opencode or other JS-based coding agents)
- CPU usage: 0.0% on idle, ~1.5% when using tools (measured on an Intel i5 7th gen, vs ~2% on idle and ~20% when working for opencode)

## Installation

### Homebrew (recommended)

```bash
brew tap gi-dellav/tap
brew trust gi-dellav/tap   # required for Homebrew 6.0.0+
brew install zerostack
# brew install multistack   # Run this to also install multistack (parallel agent manager)
```

### Script

```bash
curl -fsSL https://raw.githubusercontent.com/gi-dellav/zerostack/main/install.sh | bash
```

Or pick a tarball manually from [GitHub Releases](https://github.com/gi-dellav/zerostack/releases).

### Nix

Run directly with [`nix-run`](https://tangled.org/weethet.eurosky.social/nix-run/):

```console
$ nix-run https://github.com/gi-dellav/zerostack/archive/refs/heads/main.tar.gz
```

Add to profile:

```console
$ nix profile add --file https://github.com/gi-dellav/zerostack/archive/refs/heads/main.tar.gz
```

Add as an overlay to your system/project:

```nix
let
  pkgs = import nixpkgs {
    overlays = [
      # src thru input pinning mechanism, or use builtins.fetchTarball
      (import "${zerostack-src}/nix/overlay")
    ];
  };
in
pkgs.zerostack
```

### Cargo

```bash
# Default: loop, git-worktree, mcp, subagents, archmd
cargo install zerostack

# With all features
cargo install zerostack --all-features

# With specific features
cargo install zerostack --features acp,memory,multithread
```

Once installed, run `/prompt autoconfig` inside zerostack to explore the documentation and configure the tool interactively.

_note:_ If you have questions or you want to collaborate on the project, please join the [dedicated Matrix chatroom](https://app.element.io/#/room/#zerostack-general:matrix.org).

If you want to orchestrate multiple zerostack agents from the terminal, also install [multistack](https://github.com/gi-dellav/multistack).

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

There is also support for zerobox as an alternative sandbox backend.

## Quick start

```bash
# Set your API key (OpenRouter is default)
export OPENROUTER_API_KEY="[api_key]"

# Interactive session (default prompt: code)
zerostack

# Monochrome TUI
zerostack --no-color

# One-shot mode
zerostack -p "Explain this project"

# Continue last session
zerostack -c

# Explicit provider/model
zerostack --provider openrouter --model deepseek/deepseek-v4-flash
```

## Configuration

See [docs/CONFIG.md](docs/CONFIG.md) for config file location, accepted keys, provider
aliases, permission rules, and MCP server configuration.

You can run `/prompt autoconfig` in order to use a specialized agent that allows to navigate the documentation and customize your zerostack setup.

## Prompts system

_zerostack_ includes a set of built-in system prompts that change the agent's behavior and tone.
The idea is to build a complete suite of prompts that can fully substitute skills like [superpower](https://github.com/obra/superpowers) or the [Claude's official skills](https://github.com/anthropics/claude-plugins-official/tree/main).
You can switch between different prompts or list all registered prompts using `/prompt`.

Built-in prompts:

| Prompt                | Description                                                              |
| --------------------- | ------------------------------------------------------------------------ |
| **`code`** (default)  | Coding mode with full file and bash tool access, TDD workflow            |
| **`plan`**            | Planning-only mode — explores and produces a plan without writing code   |
| **`review`**          | Code review mode — reviews for correctness, design, testing, and impact  |
| **`debug`**           | Debug mode — finds root cause before proposing fixes                     |
| **`ask`**             | Read-only mode — only read/grep/find_files permitted, no writes or bash        |
| **`brainstorm`**      | Design-only mode — explores ideas and presents designs without code      |
| **`frontend-design`** | Frontend design mode — distinctive, production-grade UI                  |
| **`review-security`** | Security review mode — finds exploitable vulnerabilities                 |
| **`simplify`**        | Code simplification mode — refines for clarity without changing behavior |
| **`write-prompt`**    | Prompt writing mode — creates and optimizes agent prompts                |

You can also create custom prompts by placing markdown files in
`$XDG_CONFIG_HOME/zerostack/prompts/` and referencing them by name.

Additionally, the agent automatically loads `AGENTS.md` or `CLAUDE.md` from the
project root or any ancestor directory, injecting their contents into the
system prompt. When enabled (feature `archmd`), `ARCHITECTURE.md` is also loaded
the same way, providing high-level design context to speed up exploration.
Use `-n` / `--no-context-files` to disable all context file loading.

## Permission system

zerostack has five permission modes:

| Mode | CLI flag | Behavior |
|------|----------|----------|
| **restrictive** | `-R` / `--restrictive` | Ask for every operation. Config rules are ignored by default (can be enabled via `permission-modes`). |
| **readonly** | `--read-only` | Allow read/grep/find_files/list_dir. Deny writes, edits, bash, and everything else. Config rules ignored by default. |
| **guarded** | `--guarded` | Allow read tools. Ask for writes, edits, bash, and everything else. Config rules apply. |
| **standard** | (default) | Allow path tools (read/write/edit/list_dir) within CWD and subdirectories. Safe bash commands (ls, cat, git log, cargo check) auto-allowed. Ask for external paths and unrecognized commands. Config rules apply and override mode defaults. |
| **yolo** | `--yolo` | Allow everything, but prompt for destructive bash commands (rm, dd, mkfs, etc.). Config rules apply. |

The `--dangerously-skip-permissions` flag completely bypasses all permission
checks, allowing every tool operation without any guard. This is not a mode
and cannot be toggled at runtime.

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
- `/queue` — Manage input queued while the agent is busy
- `/btw` — Ask a quick side question in parallel without interrupting the agent

To see all of the commands, use `/help`.

## Input queue

You can keep typing while the agent is running. Plain text is not sent right
away and never starts a second concurrent run; it is queued and replayed as the
next prompt once the current run finishes. Each queued line is shown as
`queued: <text>`.

Manage the queue with `/queue`, which works even while a run is active:

- `/queue ls` lists the pending inputs (bare `/queue` does the same)
- `/queue clear` empties the queue
- `/queue pop` removes the last queued input, to undo a mis-typed line

Selecting `/queue` in the command picker opens a second-level menu with these
three subcommands, so you do not need to remember them.

Commands (input starting with `/`, `.`, or `!`) are not queued while a run is
active: wait for it to finish, or press Ctrl-C. Ctrl-C cancels the running agent
for real, including any child processes it spawned, and clears the queue.

## Side questions (`/btw`)

`/btw <message>` asks a quick "by the way" question in parallel with the main
agent, without interrupting it. Like `/queue`, it works even while the agent is
busy. It forks the current context (including a trace of the agent's in-flight
turn, when one is running) and answers using four read-only tools (`read`,
`grep`, `find_files`, `list_dir`); it cannot write files or run commands. It then
prints the reply inline.
Nothing is written to conversation history, and its token usage is tracked
separately in the status bar as `btw:…`. Press Ctrl-C to cancel an in-flight
`/btw` without disturbing the main agent.

You can point a question at a specific file with `@`: pick `/btw` from the
command menu, then type `@` to open the file picker (for example `/btw` then
`@src/main.rs` then "how does this work?"), and `/btw` reads the file you
reference.

## Session management

Sessions are saved to `$XDG_DATA_HOME/zerostack/sessions/`. Use `-c` to
resume the most recent session, `-r` to browse and select one, or
`--session <id>` to load a specific session.

## Memory

**NOTE:** Memory is gated behind the `memory` feature and is not included in the
default build. Install with `cargo install zerostack --features memory`.

With the `memory` feature, zerostack keeps plain-Markdown notes on disk and
injects the relevant ones into the system prompt at the start of every session,
so it remembers your preferences and recent context across runs.

Global memory files are stored in `$XDG_DATA_HOME/zerostack/agent/memory/`.

## Parallel Agent

If you want to make multiple agents work on the same repository without having to work with git worktrees,
zerostack now ships with `--parallel`, which enables full management of a temporary git worktree that will
be merged and removed before exiting the agent.

## Loop system

_zerostack_ includes an iterative coding loop for long-horizon tasks. The agent repeatedly reads the task, picks an item from the plan, works on it, runs tests, updates the plan, and loops until the task is complete or the iteration limit is reached.

**NOTE** The loop system is an _experimental_ feature.

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

| Flag                   | Description                                     |
| ---------------------- | ----------------------------------------------- |
| `--loop`               | Enable headless loop mode                       |
| `--loop-prompt <text>` | Prompt for each iteration                       |
| `--loop-plan <path>`   | Custom plan file path (default: `LOOP_PLAN.md`) |
| `--loop-max <N>`       | Maximum iterations (default: unlimited)         |
| `--loop-run <cmd>`     | Validation command to run after each iteration  |

## Git worktrees integration

_zerostack_ provides a branch-per-task workflow using git worktrees. You can create, work in, merge, and exit worktrees entirely from the chat UI.

**NOTE** The git worktrees integration is an _experimental_ feature.

### Git worktree usage

The worktrees integrations offers 3 slash commands:

| Command              | Description                                                                                                       |
| -------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `/worktree <name>`   | Create a git worktree on branch `<name>` and move into it (skips creating it if it already exists)                |
| `/wt-merge [branch]` | Merge the worktree branch into `[branch]` (default: `main`/`master`), push, clean up, and return to the main repo |
| `/wt-exit`           | Return to the main repo without merging                                                                           |

### Example workflow for git worktrees

1. **Create** — `/worktree feature-x` creates a new branch and worktree directory and moves you there.
2. **Work** — Use zerostack normally; changes stay on the feature branch.
3. **Merge** — `/wt-merge` tells the agent to merge the branch, push, clean up, and return to the main repo.
4. **Exit** — `/wt-exit` immediately returns to the main repo without merging.

### Auto-merge on exit

When you quit zerostack while in a worktree, the `--wt-auto-merge` flag
(or `--parallel`, which implies it) causes zerostack to attempt merging the
worktree branch before exiting.

- **Clean merge**: completes silently (merge, push, remove worktree, delete branch).
- **Merge conflicts**: zerostack lists conflicting files and prompts:
  ```
  [a]bort  [l]eave for manual resolution  [h]elp (agent resolves)
  ```
  - `a` – abort the merge, restore clean state, do not delete the worktree.
  - `l` – leave the conflict state in the main repo for manual `git mergetool`.
  - `h` – abort the merge, then spawn the agent to redo the merge with
    interactive conflict-resolution guidance (same as `/wt-merge`).

| Flag | Description |
|------|-------------|
| `--worktree <name>` | Create a worktree on branch `<name>` and `cd` into it. |
| `--wt-auto-merge`   | Auto-merge worktree branch on exit. |
| `--parallel`        | Create a timestamped worktree with auto-merge on exit. |
| `--wt-force`        | Force worktree remove and branch delete (`-D`) even if dirty. |
| `--wt-base-dir <dir>` | Base directory for worktrees (default: parent of repo). |

## ACP (Agent Communication Protocol) support

**ACP** is a JSON-RPC based protocol that standardizes communication between code editors
(IDEs, text-editors, etc.) and coding agents. With the `acp` feature enabled, zerostack
acts as an ACP **Agent** server, allowing editors like **Zed** to connect to it as a
coding agent backend.

**NOTE:** ACP support is gated behind the `acp` feature and is not included in the
default build.

### ACP usage

```bash
# Start zerostack in ACP stdio mode (editor spawns this as a subprocess)
zerostack --acp

# Start zerostack in ACP TCP mode (listen on 0.0.0.0:7243)
zerostack --acp --acp-host 0.0.0.0 --acp-port 7243
```

### ACP config

In `~/.local/share/zerostack/config.json`:

```json
{
  "acp_servers": {
    "my-editor": {
      "host": "127.0.0.1",
      "port": 7243
    }
  }
}
```

ACP mode requires setting up an LLM provider (the standard `--provider`, `--model`,
and API key env vars apply). Without it, zerostack cannot process prompts.

## Supported providers

- OpenRouter (default)
- OpenAI-compatible (vLLM, LiteLLM, etc.)
- Anthropic
- Gemini
- Ollama

Custom providers can be configured with any base URL and API key environment
variable in `~/.local/share/zerostack/config.json`.

## License

GPL-3.0-only
