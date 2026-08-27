---
description: "How ARCHITECTURE.md files give the zerostack agent and its subagents high-level design context about your project."
---

# ARCHITECTURE.md

zerostack supports an optional `ARCHITECTURE.md` file that gives both the main
agent and exploration subagents high-level design context about your project.

## What It Does

When `ARCHITECTURE.md` files exist (at the project root and/or in parent
directories), their content is appended to the agent's system prompt preamble
— right after `AGENTS.md` context and before the custom prompt. This means
every LLM call carries awareness of your project's architecture.

**All subagents also receive the same architecture context**, so they can
explore the codebase with an understanding of the overall design.

## Why Use It

### For the Agent

Without `ARCHITECTURE.md`, an agent exploring a large codebase starts with
zero architectural knowledge. It reads `AGENTS.md` for conventions, then
begins probing files one by one. This works but costs tokens and turns as
the agent builds a mental model from scratch.

With `ARCHITECTURE.md`, the agent enters the conversation already knowing:
- How the project is organized (directory layout, module responsibilities)
- Key types, traits, and data structures
- Where control flow and data flow live
- Design decisions and constraints
- External dependencies and their roles
- Entry points and how things boot

This front-loads understanding, reducing the number of read probes needed and
making the agent's first responses more accurate.

### For the User

- **Consistency across sessions** — the agent stays aligned with your design
  intent across different conversations
- **Better subagent delegation** — when using the `task` tool, subagents
  understand the architecture without querying the main agent
- **Onboarding** — new contributors (human or AI) get a structured overview
- **Living documentation** — the agent can (and is prompted to) update
  `ARCHITECTURE.md` when significant changes are made

## Discovery and Loading

zerostack loads `ARCHITECTURE.md` files using the same recursive upward search
as `AGENTS.md`:

1. **Global**: `~/.local/share/zerostack/agent/ARCHITECTURE.md` (XDG data dir)
2. **Project**: `ARCHITECTURE.md` in the current working directory and all
   parent directories up to the filesystem root

Files from all levels are concatenated, with source-path headers indicating
where each block came from. This lets you define organization-wide conventions
in the global file while having project-specific architecture in each repo.

At startup, if no `ARCHITECTURE.md` is found anywhere in the directory tree,
zerostack offers to create one:

```
No ARCHITECTURE.md found in /home/you/project. Create one? [y/N]
```

If you answer yes, a template is written to the project root. The template
includes sections for directory layout, key types/traits, control flow, data
flow, design decisions, dependencies, and entry points.

When you accept and the template is created, zerostack automatically injects
a system message instructing the agent to explore the codebase and populate
the file with a thorough architectural overview.

### Template Contents

The generated template contains:

```markdown
# Architecture Overview

## Directory Layout
<!-- Describe the top-level directory structure and responsibilities -->

## Key Types / Traits
<!-- List the primary data structures, traits, and their relationships -->

## Control Flow
<!-- How does execution flow through the system? -->

## Data Flow
<!-- How does data move through the system? -->

## Design Decisions
<!-- Notable architectural choices and tradeoffs -->

## Dependencies
<!-- Key external dependencies and their roles -->

## Entry Points
<!-- How does the application start and accept input? -->
```

## Disabling

Pass `--no-context-files` (or `-n`) to suppress loading of both `AGENTS.md`
and `ARCHITECTURE.md`. You can also set `no_context_files = true` in your
config file.

## How It Integrates

| Layer | Behavior |
|---|---|
| **System prompt** | Architecture content appended after `AGENTS.md`, before custom prompt |
| **Subagents** | Each subagent receives the architecture context in its preamble |
| **`task` tool** | Exploration subagents instructed to read `ARCHITECTURE.md` first |
| **TUI status** | Displays `loaded ARCHITECTURE.md` when architecture content exists |
| **Prompts** | Built-in prompts reference architecture-aware workflows |

## Writing a Good ARCHITECTURE.md

A well-written `ARCHITECTURE.md` should be **concise** (aim for 200-500 words
for small projects, 500-2000 for larger ones) and **actionable** — think of it
as a cheat sheet the agent can reference when making decisions. Avoid
reproducing code; focus on structure, relationships, and rationale.

### Recommended Sections

1. **Directory Layout** — one-line summaries of each top-level directory
2. **Key Types/Traits** — the 5-10 most important data structures
3. **Control Flow** — request lifecycle, main loops, async boundaries
4. **Data Flow** — how data enters, transforms, and exits the system
5. **Design Decisions** — "why X instead of Y" for critical choices
6. **Dependencies** — key libraries and what they're used for
7. **Entry Points** — binary entry, API handlers, CLI parsing

### Example

```markdown
# Architecture Overview

## Directory Layout
- `src/agent/` — Agent building, prompt construction, tool execution
- `src/ui/` — TUI event loop, renderer, input handling, slash commands
- `src/provider/` — LLM provider abstraction (OpenAI, Anthropic, etc.)
- `src/config/` — Config parsing, validation, resolution
- `src/extras/` — Optional features gated behind Cargo features

## Key Types
- `AnyAgent` / `AnyClient` — type-erased agent and LLM client
- `Session` — conversation state, messages, tokens, compactions
- `ContextFiles` — loaded AGENTS.md, ARCHITECTURE.md, prompts, themes

## Control Flow
1. CLI args parsed → config loaded → context files discovered
2. Agent built with system prompt (agents + architecture + prompt)
3. TUI event loop: user input → agent runner → streaming events → renderer
4. Slash commands intercept input starting with `/`

## Data Flow
- User input → InputEditor → event loop → agent.spawn_runner()
- Runner streams AgentEvent (Token, Reasoning, ToolCall, ToolResult, Done); ToolCall/ToolResult carry a correlation `call_id` (rig's `internal_call_id`) so a parallel batch pairs each result with its own call
- Events rendered incrementally via Renderer::write_line()

## Design Decisions
- Type-erased client/agent via trait objects for provider flexibility
- Tokio for async I/O; crossterm for TUI
- mpsc channels for agent events, user events, and permission requests

## Dependencies
- `rig` / `rig-core` — LLM client abstraction
- `crossterm` — cross-platform terminal manipulation
- `tokio` — async runtime
- `serde` / `toml` — config parsing

## Entry Points
- `main.rs` — binary entry, CLI parsing, session init
- `run_interactive()` — TUI main loop
```

## Comparison with AGENTS.md

| Aspect | AGENTS.md | ARCHITECTURE.md |
|---|---|---|
| **Purpose** | Coding conventions, instructions, project-specific procedures | High-level design: structure, relationships, rationale |
| **Scope** | "How to work in this codebase" | "How this codebase is built" |
| **Update frequency** | Rare (conventions change slowly) | With significant refactors or new modules |
| **Typical length** | Short to medium | Medium (200-2000 words) |
| **Loaded together** | Yes, both concatenated into system prompt preamble | |

Both files complement each other: `AGENTS.md` tells the agent how to operate;
`ARCHITECTURE.md` tells it what it's operating on.
