# Slash Commands

All slash commands are available from the TUI input prompt.

## Session

| Command | Description |
| ------- | ----------- |
| `/clear` | Clear the current session (all messages, tokens, compactions). |
| `/undo` | Remove the last exchange (user message + assistant response). |
| `/retry` | Load the last user message into the input editor for editing. |
| `/quit` | Exit zerostack. |
| `/sessions` | List recent saved sessions (up to 20). |
| `/sessions <id-prefix>` | Load a session by its ID prefix. |
| `/sessions delete <id-prefix>` | Delete a session by its ID prefix. |
| `/history` | Show global chat history (last 10 entries across sessions). |

## Provider & Model

| Command | Description |
| ------- | ----------- |
| `/provider` | Show the current provider. |
| `/provider <name>` | Switch to a different provider. |
| `/model` | Show the current model. |
| `/model <name>` | Switch to a different model. |
| `/models` | List all quick models defined in config. |
| `/models <name>` | Switch to a named quick model. |
| `/models-add <name> <provider> <model>` | Save a new quick model to the config file. |

## Context Files

| Command | Description |
| ------- | ----------- |
| `/add` | List files currently added to context (with sizes). |
| `/add <path>` | Add a file to the agent's context (absolute or relative path). |
| `/drop <path>` | Remove a file from the agent's context. |
| `/drop-all` | Remove all added files from the agent's context. |

Files added with `/add` are included alongside the conversation in each request,
useful for giving the agent reference documentation or code without cluttering
the chat directly.

## Initialization

| Command | Description |
| ------- | ----------- |
| `/init` | Create an AGENTS.md file for the current project by delegating to the agent. |
| `/init force` | Overwrite the existing AGENTS.md if one already exists. |

Requires a `code` prompt to be configured (run `/regen-prompts` to restore
built-in prompts, or create a custom `code.md` prompt).

## Security

| Command | Description |
| ------- | ----------- |
| `/mode` | Show the current security mode. |
| `/mode standard` | Allow path tools within CWD, ask for external paths. Config rules apply. |
| `/mode restrictive` | Ask for every operation. Config rules skipped. |
| `/mode readonly` | Allow reads only; deny writes, edits, bash, and everything else. |
| `/mode guarded` | Allow reads; ask for writes, edits, bash, and everything else. Config rules apply. |
| `/mode yolo` | Allow everything; ask for destructive bash commands. Config rules apply. |

Prompts can set the security mode automatically via `%%mode=<mode>` on
the first line. When a prompt with `%%mode=last_user_mode` is activated,
the mode reverts to whatever was last set explicitly by `/mode` or
startup config. See Prompts & Themes below.

## Prompts & Themes

| Command | Description |
| ------- | ----------- |
| `/prompt` | List available prompts. |
| `/prompt <name>` | Activate a named prompt. Also applies `%%mode=` from the prompt file if present (see below). |
| `/prompt default` | Clear the active prompt. |

Prompts may include a `%%mode=<mode>` directive on the **first line** to
automatically switch the security mode when activated. Valid modes:
`standard`, `restrictive`, `readonly`, `guarded`, `yolo`. Use
`%%mode=last_user_mode` to restore the mode the user last set via `/mode`
or startup config. The directive line is stripped from the prompt content
before it reaches the agent.

Example `ask.md`:
```markdown
%%mode=readonly

## Read-Only Mode

You are in read-only mode. Only read files and explore.
```
| `/theme` | List available themes. |
| `/theme <name>` | Activate a named theme. |
| `/theme default` | Clear the active theme (use config colors). |
| `/regen-prompts` | Restore built-in prompts to the prompts directory. |
| `/regen-themes` | Restore built-in themes to the themes directory. |

## Conversation

| Command | Description |
| ------- | ----------- |
| `/compress [instructions]` | Compress conversation history to free context window space. |
| `/compact` | Alias for `/compress`. |
| `/editsys` | Show the current edit system mode (similarity or hashedit). |
| `/editsys similarity` | Use SEARCH/REPLACE with fuzzy matching for edits (default). |
| `/editsys hashedit` | Use CRC-32 tag-based edits (token-efficient, CAS-guarded). |
| `/btw <message>` | Ask a quick side question in parallel, without touching the main conversation. It forks the current context (including the main agent's in-flight turn, if any), answers using read-only tools (read/grep/find_files/list_dir, no writes or bash), and prints the answer inline. Works even while the main agent is running. Nothing is written to history; its token cost is shown separately as `btw:$…`. Ctrl-C cancels an in-flight `/btw` without disturbing the main agent. |
| `/reasoning` | Toggle LLM reasoning on/off (requires model support). |
| `/thinking` | Alias for `/reasoning`. |
| `/toggle` | Show available toggleable features. |
| `/toggle todo [on\|off]` | Enable or disable todo-list tools. |

## Memory (feature-gated)

Requires building with `--features memory`.

| Command | Description |
| ------- | ----------- |
| `/memory` | Show memory status (MEMORY.md, scratchpad, daily log). |
| `/memory status` | Same as `/memory` (explicit status check). |
| `/memory search <query>` | Search all memory files with case-insensitive keyword matching. |
| `/memory read long_term` | Read the global MEMORY.md file. |
| `/memory read scratchpad` | Read the project scratchpad (open checklist items). |
| `/memory read daily [date]` | Read a daily log (defaults to today; use YYYY-MM-DD for past). |
| `/memory read note <name>` | Read a named note. |
| `/memory write long_term <content>` | Append to the global MEMORY.md. |
| `/memory write scratchpad <content>` | Append to the project scratchpad. |
| `/memory write daily <content>` | Append to today's daily log. |
| `/memory write note:<name> <content>` | Append to a named note. |
| `/memory editor` | Open MEMORY.md in your system `$EDITOR`. |
| `/memory clear scratchpad` | Clear all scratchpad items. |
| `/memory clear daily` | Clear all of today's entries. |

Long-term memory (MEMORY.md) and open scratchpad items are automatically injected
into every request. Daily logs (today + yesterday) are also included. Notes and
older daily logs are accessible via `/memory read` and `memory_search`.

## MCP (feature-gated)

| Command | Description |
| ------- | ----------- |
| `/mcp` | List connected MCP servers and their tool counts. |
| `/mcp <server>` | List tools of a specific MCP server. |

## Worktree (feature-gated)

| Command | Description |
| ------- | ----------- |
| `/worktree <name>` | Create a git worktree on a new branch and `cd` into it. |
| `/wt-merge [branch]` | Merge the worktree branch back into the target branch. |
| `/wt-exit` | Exit the worktree and return to the main repo. |

## Loop (feature-gated)

| Command | Description |
| ------- | ----------- |
| `/loop [prompt]` | Start the iterative coding loop. |
| `/loop stop` | Stop the active loop. |
| `/loop status` | Show current loop status. |

## Shell Commands

Prefix a message with `!` to run it as a shell command instead of sending it to
the agent. The command's output is captured and stored in the session history as
an Assistant message. Works in both TUI and `--print` mode.

| Example | Description |
| ------- | ----------- |
| `!ls -la` | List files in the current directory. |
| `!git status` | Check git status without involving the agent. |
| `!cargo test` | Run tests and capture the output. |
| `!` | Empty command shows an error. |

If you want to run a command and then discuss the output with the agent, just
type `!<command>` first (it stores the output as an Assistant message), then
follow up with a normal message asking the agent about it.

## Prompt Shortcut

Prefix a message with `.` to quickly switch prompts or run a one-shot query with
a different prompt.

| Example | Description |
| ------- | ----------- |
| `.` | Open the prompt picker (same as `/prompt` picker). |
| `.ask` | Switch to the `ask` prompt (same as `/prompt ask`). |
| `.plan what files changed?` | Temporarily use the `plan` prompt for this query, then restore the previous prompt and security mode. |

The `.[prompt] [msg]` syntax is a one-shot: it sets the prompt, submits the
message, and after the response restores the previous prompt and
`last_user_mode`.

## General

| Command | Description |
| ------- | ----------- |
| `/help` | Show the full help message listing all commands and keybindings. |

## Keybindings

| Shortcut | Action |
| -------- | ------ |
| `Enter` | Send message. |
| `Shift+Enter` | Insert newline. |
| `Ctrl+C` | Cancel current agent response or quit. |
| `Ctrl+D` | Send message (alternative). |
| `Ctrl+W` | Delete word backwards. |
| `Ctrl+U` | Delete to beginning of line. |
| `Ctrl+L` | Clear terminal. |
| `Ctrl+G` | Open the current input in the system editor (`$EDITOR`). |
| `Ctrl+H` | Launch `lazygit` (git TUI) in the project directory. |
| `Ctrl+S` | Save session. |
| `Tab` | Activate file picker / auto-complete paths. |
| `Up / Down` | Navigate command history. |
| `PageUp / PageDown` | Scroll viewport. |
| `Home / End` | Jump to start/end of input. |
| `Alt+Enter` | Retry last prompt. |
| `Escape` | Close active picker / cancel. |
