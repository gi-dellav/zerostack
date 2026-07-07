# Part 1. Get Started

Thanks for picking up zerostack, here you can find a easy guide on installing zerostack, setting up a model, and getting to know the basic commands.

This tutorial is designed to work on any Linux, macOS and WSL environment; if you are using Windows, we recommned using WSL, as Windows support is not currently mantained.

## 1. Installation

You can install via Homebrew, Cargo or Shell; most likely, you want to install via Shell:
```
curl -fsSL https://raw.githubusercontent.com/gi-dellav/zerostack/main/install.sh | bash
```

For Homebrew and Cargo, the instructions are:
```
# Homebrew
brew tap gi-dellav/tap
brew trust gi-dellav/tap   # required for Homebrew 6.0.0+
brew install zerostack

# ----
# Cargo
cargo install zerostack
```

## 2. Setting up the provider

zerostack defaults to **OpenRouter**, which is a provider that gives access to hundreds of models through a single API key, without needing per-provider signup; OpenRouter also offers the great advantage of connecting to free models, which we'll use for completing this setup.

### Get an OpenRouter API key

1. Go to [openrouter.ai/keys](https://openrouter.ai/keys)
2. Create a key
3. Set it as an environment variable:

```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
```

Add that line to `~/.bashrc` or `~/.zshrc` to make it permanent.

### Or use another provider

While we showed OpenRouter, you can set up all mainstream provider, local inference engines, and all OpenAI-compatible servers.

You can just set the matching env var with :

| Provider   | Env var               |
| ---------- | --------------------- |
| OpenAI     | `OPENAI_API_KEY`      |
| Anthropic  | `ANTHROPIC_API_KEY`   |
| Gemini     | `GEMINI_API_KEY`      |
| Ollama     | (none — local)        |

Then, you can change your configuration file (`~/.local/share/zerostack/config.toml` on Linux/WSL or `~/Library/Application Support/zerostack/` on macOS) by adding `provider = [provider_name]` in order to change your default provider.

If you are using a provider that's not your default one, use the `--provider` CLI flag:

```bash
zerostack --provider anthropic
```

See [Providers](PROVIDERS.md) for custom endpoints, header configuration, and prompt caching details.

## 3. Pick a Model

OpenRouter models use the format `provider/model-name`: [here](https://openrouter.ai/models?order=top-weekly) you can find the currently most used models, and [here](https://openrouter.ai/models?order=top-weekly&max_price=0) you can list only free models sorted by usage.

Models can be changed using the provider's model name via the `/model` command.

## 4. How to use Quick Models

Using model strings is kinda annoying, as it requires for you both to insert long model names and to be using the correct provider: that's why zerostack implements Quick Models, an alias that sets both the model and the provider that the agent must used.

A quick model can be added in the configuration by doing something like:
```
[quick_models.fast]
provider = "openrouter"
model = "deepseek/deepseek-v4-flash"
```

From there, you can use the `model` field in the configuration file to set the default model, or use `/models` to use an interactive picker directly in the agent.

## 5. Start a Session

You are now ready to launch zerostack! Just run `zerostack` and look at the beautiful TUI; type a message, press Enter and the agent will responds with streaming tokens.

It can read, write, edit, and search your codebase, while giving full control to the user on what it's allowed to do.

# Part 2. Useful commands

Now that you are in, you might want to be able to control the agent, and here's how:

## 1. Essential commands

By pressing `/` on an empty message, you can select any command to send the agent; here are the most useful commands:

| Command | What it does |
| ------- | ------------ |
| `/help` | List all commands |
| `/models <name>` | Switch model mid-session using Quick Models |
| `/clear` | Start with a fresh context |
| `/mode readonly` | Lock down to read-only |
| `/undo` | Undo the last exchange |
| `/redo` | Redo the last undo action |
| `/btw` | Ask a question to the agent without changing the context |
| `/review` | Ask the agent to review the last changes made |
| `/sessions` | List older sessions |
| `/rename <name>` | Rename the current session |
| `/quit` | Exit |

## 2. Prompts

Prompts change *how* the agent behaves. Type `.` at the start of a message to one: if it's followed by some text, the prompt will be used only for that message; if not, it will be set as the default for the rest of the session.

| Prompt | Use for |
| ------ | ------- |
| `code` | Writing and editing code (default) |
| `plan` | Designing before writing |
| `review` | Reviewing changes |
| `ask` | Q&A — no tools, just answers |
| `brainstorm` | Ideation, exploring options |
| `debug` | Systematic debugging |
| `refactor` | Restructuring existing code |

## 3. Autoconfig

There is one special prompt, called `autoconfig`, that has full access to your zerostack configuration and to the project's documentation: after reading this Get Started guide, you might decide to just never read any documentation or any configuration file for zerostack, you just need to load `autoconfig` and everything will be managed.

## 4. Keybindings

Here is some keybindings to speed up your coding experience:

| Keys | Action |
| ---- | ------ |
| `Ctrl+R` | Toggle reasoning/thinking |
| `Ctrl+G` | Open input in `$EDITOR` |
| `Ctrl+H` | Launches `lazygit`
| `Ctrl+S` | Force-save session |
| `Ctrl+C` | Interrupt the agent |
| `PgUp` / `PgDn` | Scroll chat |
| `Home` / `End` | Jump to top/bottom |

## 5. CLI flags

If you want to use zerostack from scripts, from other programs, or if you just want to load up the agent in whatever way you prefer, there are some CLI flags you might want to know about:

| Flag | Action |
| ---- | ------ |
| `-p <msg>` | Sends a message |
| `-c` | Continues from last open session |
| `--name <name>` | Set a name for the new session |
| `--session <id-or-name>` | Load session by ID prefix or name |
| `--read-only` | Only reads files |
| `--yolo` | No limitations given to the agent |
| `--sandbox` | Run the agent inside a sandbox (Experimental) |
| `--worktree` | Run the agent inside a git worktree (Experimental) |
| `--parallel` | Run the agent inside a self-managed git worktree (Experimental) |
| `--load-prompt <prompt>` | Use a specific prompt |

# Conclusions

Thanks for reading the *Get Started* guide until the end!

I hope that you liked reading about zerostack, and that you'll enjoy using this agent; you can discover more about the configuration abilites and the integrated commands of the agent by reading the documentation, by asking questions to the `autoconfig` prompt, or by experimenting with the `/` interactive picker.

---

Cheers,
Giuseppe Della Vedova
