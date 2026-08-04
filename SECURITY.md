# Security Policy

## Reporting a vulnerability

Please report security issues privately through GitHub private vulnerability
reporting: open the [Security tab](https://github.com/gi-dellav/zerostack/security/advisories/new)
of the repository and choose "Report a vulnerability". Do not open a public
issue or pull request for a vulnerability.

Useful things to include: the zerostack version (`zerostack --version`), the
platform, the configuration involved (with API keys removed), and the smallest
reproduction you have.

Fixes ship in the next release from `main`. There are no long-term support
branches, so please upgrade to the latest release before reporting.

## Bash sandbox security model

zerostack can run the commands issued by the `bash` tool inside an isolated
environment. This is opt in: `--sandbox` on the command line, or `sandbox =
true` in the config file. The backend is selected with `--sandbox-backend` /
`sandbox-backend`: `bwrap` ([bubblewrap](https://github.com/containers/bubblewrap),
the default, Linux only) or `zerobox` ([zerobox](https://github.com/afshinm/zerobox),
macOS and Linux, installable with `cargo install zerobox` among others).

The sandbox exists to contain accidental damage from commands the model
proposes. It is not a full security boundary against an adversary who controls
the command being run.

### What it protects against

With the default `bwrap` backend, a sandboxed command sees:

- `/` bind mounted read only, so writes outside the allowed paths fail
- the current working directory bind mounted read write, so the project you are
  working in stays editable
- a fresh `/tmp` (tmpfs), a private `/proc`, and a minimal `/dev`
- separate IPC, PID, UTS, and cgroup namespaces, so the command cannot see or
  signal processes outside the sandbox
- a cleared environment, repopulated with an allowlist of common variables
  (`PATH`, `HOME`, toolchain and locale variables, and so on)
- `--die-with-parent`, so the sandboxed process tree does not outlive zerostack

In practice this stops the common accidents: a stray `rm -rf` outside the
project, an installer writing into system directories, a build script editing
files elsewhere on the machine.

The `zerobox` backend is shaped differently. zerobox is powered by the OpenAI
Codex sandbox runtime and denies writes, network access, and environment
variables by default, with network access grantable per domain. zerostack
invokes it as `zerobox --allow-write <cwd> -- <shell> -c <command>`, so the only
hole zerostack opens in that policy is write access to the working directory,
and everything else follows zerobox's own defaults.

### What it does not protect against

These are known gaps, listed so you can decide whether the sandbox is enough for
your situation. Most of them come from how zerostack configures the default
`bwrap` backend; the last group applies whichever backend you pick.

With the `bwrap` backend:

- **Network access is fully open.** zerostack does not unshare the network
  namespace, so a sandboxed command can reach the internet and your local
  network, which means it can exfiltrate anything it can read. The `zerobox`
  backend behaves differently here: it denies network access by default under
  its own policy.
- **Most of your home directory is still readable.** `/` is mounted read only,
  not hidden, so everything under `$HOME` is visible inside the sandbox except
  the nine credential directories masked by default: `~/.ssh`, `~/.aws`,
  `~/.gnupg`, `~/.kube`, `~/.docker`, `~/.config/gh`, `~/.config/gcloud`,
  `~/.config/op`, `~/.config/sops/age`. Each of these, when present on the
  host, is covered by a tmpfs, so a sandboxed command sees it as an empty
  directory rather than reading your keys and tokens. Use `sandbox-expose`
  (config key or repeatable `--sandbox-expose <path>` flag) to restore
  read-only access to a masked entry or a subpath of one when a command
  legitimately needs it; see `docs/CONFIG.md`. Two gaps remain: single-file
  credentials such as `~/.netrc` and `~/.npmrc` have no clean bwrap
  file-hiding primitive and are not masked, and everything else under `$HOME`
  outside the nine directories above (`.env` files, browser profiles, shell
  history) stays fully readable.
- **The whole user cache directory is writable.** `~/.cache` (or
  `$XDG_CACHE_HOME`) is bind mounted read write so that build tooling works.
  Anything cached there, including tool caches other programs trust, can be
  modified.
- **The advertised agent socket is masked, not every possible agent.**
  `SSH_AUTH_SOCK` and `SSH_AGENT_PID` are removed from the sandbox environment
  allowlist, and the socket path the host's `SSH_AUTH_SOCK` points to is bound
  over with `/dev/null`, so a sandboxed command cannot reach the agent even by
  reconstructing the variable by hand. This targets the socket the host
  advertises: a secondary agent socket running elsewhere (for example a
  systemd `ssh-agent.socket` alongside gnome-keyring) can remain reachable
  under `/run/user`. The gpg-agent SSH socket under `~/.gnupg` is covered by
  the directory mask above. There is no in-sandbox switch to re-enable the
  agent; recovery paths are `sandbox-expose ~/.ssh` for direct key-file auth
  (passphrase-less keys) and the `!` prefix, which runs the command outside
  the sandbox with the agent intact.
- **Kernel level escapes are out of scope of this design.** bubblewrap uses user
  namespaces; a kernel vulnerability, or a host configured to grant more than the
  usual namespace privileges, can defeat the isolation.

With any backend:

- **Only the `bash` tool is sandboxed.** File reads and writes performed by
  zerostack's own tools, MCP servers, hooks, and shell commands you run yourself
  with the `!` prefix all run outside the sandbox. They are governed by the
  permission system, not by this isolation.
- **Backend availability depends on the platform.** `bwrap` is Linux only, so on
  macOS the default backend is missing and `sandbox = true` alone gives you no
  isolation: commands run bare, with a warning in the logs. Install zerobox and
  set `sandbox-backend = "zerobox"` for real isolation on macOS, or set
  `sandbox-required` so those commands are refused instead of run bare.
- **zerostack does not verify what the backend enforces.** It launches the
  backend with the arguments described here and trusts the result. What this
  document says about zerobox is its documented default behavior, not the result
  of an audit of its implementation.

Treat the sandbox as a seatbelt against mistakes, not as a container for
untrusted code. If you need a real boundary, run zerostack itself inside a VM or
a container with the network and credentials you are willing to expose.

### Per-backend boundaries

| Backend | Isolation |
| --- | --- |
| `bwrap` (default) | Linux only. The bubblewrap mounts and namespaces described above, with the network left open. The nine built-in credential directories are masked by default and the advertised ssh-agent socket is cut off; see above. |
| `zerobox` | macOS and Linux. Denies writes, network access, and environment variables by default, with per-domain network allowances. zerostack invokes `zerobox --allow-write <cwd> -- <shell> -c <command>`, so the working directory is writable and the rest of the policy is whatever zerobox enforces. Credential masking does not apply here: zerobox exposes no mount-policy surface to inject it, and whether its own defaults limit reads under `$HOME` has not been verified. |
| none | With the sandbox off (the default), bash commands run directly as your user with no isolation at all. The permission system is the only gate. |

## Best effort versus guarantee

Two different contracts:

- `sandbox = true` (or `--sandbox`) is **best effort**. If the backend binary is
  not installed, zerostack logs a warning at startup and each command runs
  unsandboxed rather than failing. Sessions keep working on machines without the
  backend, but with no isolation.
- `sandbox-required = true` (or `--sandbox-required`) is the **guarantee**. When
  the backend binary is unavailable, bash commands are refused with an error
  that says why, instead of running bare. `sandbox-required` implies `sandbox`.

`sandbox-required` does not exit at startup. Everything else in the session
(reading files, editing, planning) keeps working, only bash execution is
refused. This is the setting to use for unattended or automated runs, where
nobody is watching the log for the "running unsandboxed" warning.

Neither setting changes the gaps listed above. `sandbox-required` guarantees
that the isolation is present, not that the isolation is complete.
