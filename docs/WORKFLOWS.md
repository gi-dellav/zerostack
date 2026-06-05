# Workflows & Macros

Workflows let you define reusable sequences of slash commands and messages
that the agent executes as a unit. Each workflow file becomes a custom slash
command (a *macro*) that you can invoke from the TUI.

## How They Work

1. Create a `.workflow` text file in one of the workflow directories.
2. The file name (without `.workflow`) becomes the slash command name.
3. Type `/<name>` to run the workflow.

## Workflow File Format

Each line in a workflow file is interpreted as follows:

| Line starts with | Meaning |
|---|---|
| `#` | Comment (ignored) |
| `/` | Slash command (executed in order before the message) |
| Anything else | Message text (sent to the agent as a single prompt) |
| Blank line | Ignored |

Slash commands run first, in order. If the workflow has a message, it is sent
to the agent after all slash commands complete. A workflow can contain only
slash commands (e.g. to switch models and prompts) or only a message, or both.

## Directory Locations

Workflows are loaded from two directories (project-local overrides global):

1. **Project-local**: `.zerostack/workflows/` (in the current project root)
2. **Global**: `~/.config/zerostack/workflows/`

## Example Workflows

### Review Changes

File: `.zerostack/workflows/review.workflow`

```
/prompt code
/model claude

Review the current git diff for bugs, security issues, and style problems.
For each issue found, suggest a fix with code examples.
Priority order: bugs > security > style.
```

Invoke with: `/review`

### Explain Code

File: `~/.config/zerostack/workflows/explain.workflow`

```
# Switch to the "ask" prompt (concise, explanatory)
/prompt ask

Explain what the current changes do in simple terms.
Focus on the high-level purpose, not implementation details.
```

Invoke with: `/explain`

### Switch Settings Only (no message)

File: `.zerostack/workflows/code.workflow`

```
# Configure for coding session
/model claude
/prompt code
/mode standard
```

Invoke with: `/code` — changes settings without sending a message.

## Listing Workflows

```text
/workflow        alias for /workflow list
/workflow list   show all available workflows
```

Displays each workflow's name and file path.

## Notes

- Workflows are reloaded from disk on each invocation — no need to restart
  zerostack after editing a workflow file.
- Project-local workflows shadow global ones with the same name.
- Workflow files must have the `.workflow` extension.
- The feature is enabled by default. If disabled, rebuild with:
  `cargo install --path . --debug --features workflows`
