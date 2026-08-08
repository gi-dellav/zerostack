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
  signal processes outside the sandbox, plus a separate network namespace when
  `sandbox-network = false` (see below; the default keeps the host network)
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

- **Network access is open by default.** Out of the box zerostack does not
  unshare the network namespace, so a sandboxed command can reach the internet
  and your local network, which means it can exfiltrate anything it can read.
  Set `sandbox-network = false` (or pass `--sandbox-network=false`) to unshare
  it. Each bash call then runs in a fresh network namespace holding nothing but
  its own private loopback device, which bubblewrap brings up for it: a server
  the command starts and then talks to on `127.0.0.1` still works, because both
  ends are inside that namespace, while the internet, the LAN, and anything
  listening on the *host's* loopback (a dev server on `localhost:3000`, a
  database, a local registry) are all unreachable. This is about routing only:
  `/sys` is a host bind, so interface names and MAC addresses remain readable
  from inside the offline namespace (`/proc` is remounted fresh and reflects
  the new namespace, not the host's). The namespace lasts as long
  as the command, so a server backgrounded by one bash call is gone for the
  next one. Unsharing the network also replaces the abstract Unix socket
  namespace, so abstract-address sockets on the host, which some D-Bus and X11
  setups use, are cut off with it. Unix sockets that live at a filesystem path
  are not cut off; see the next gap. Turning this on is what makes credential
  masking worth more than it is alone: masking limits what a sandboxed command
  can read but leaves it a way out, and an offline namespace removes the way
  out but leaves everything outside the masked directories readable. Together
  they narrow the path from "reads your secrets" to "sends them somewhere"
  **for the `bash` tool only**, which is the scope of every claim in this
  section: see "Only the `bash` tool is sandboxed" below for the file reads,
  MCP servers, hooks, and `!` commands that never enter the sandbox at all.
  Two things bound that narrowing, and neither is a detail. On a machine
  running Docker, a session bus, or any other daemon that listens on a socket
  file, the next gap applies and the way out is still open, so the narrowing
  is not claimed there at all. And the tool result is itself a way out: a
  sandboxed command's stdout and stderr are what the `bash` tool returns, and
  that text is sent to whichever model provider the session is configured
  with, so anything a command prints has left your machine no matter what the
  network setting says. Turning the network off removes the command's own
  network access; it cannot close the channel the agent itself runs on.
  And without `sandbox-required`, a missing backend still falls back to
  running commands bare, with the network intact and nothing masked. The
  `zerobox` backend is unaffected by this key and denies network access by
  default under its own policy.
- **Unix sockets on disk stay connectable, even with the network unshared.**
  A read-only bind mount stops writes to a socket *file*, but it does not stop
  a `connect(2)` to it: the kernel's read-only protection covers regular
  files, directories, and symlinks, not connecting to a socket inode. Socket
  paths under `/run`, `/var/run`, and `/run/user/$UID` are therefore visible
  and usable from inside the sandbox whatever `sandbox-network` is set to,
  because they are filesystem objects rather than network ones and live
  outside the network namespace entirely. The sharpest case is
  `/run/docker.sock` on a host running Docker: a sandboxed command that can
  talk to the Docker daemon can ask it to start a container with the host
  filesystem mounted and the host network attached, which is a full escape
  from both the mounts and the network namespace, credential masks included.
  A session bus at `/run/user/$UID/bus` is the same shape at a smaller scale,
  and so is any other daemon socket your distribution leaves there. zerostack
  does not mask host socket paths today. Doing so is plausible future
  hardening, not current behavior, so treat "the sandbox is offline" as a
  statement about the network stack and not about every way out of the
  machine.
- **Most of your home directory is still readable.** `/` is mounted read only,
  not hidden, so everything under `$HOME` is visible inside the sandbox except
  the nine credential directories masked by default: `~/.ssh`, `~/.aws`,
  `~/.gnupg`, `~/.kube`, `~/.docker`, `~/.config/gh`, `~/.config/gcloud`,
  `~/.config/op`, `~/.config/sops/age` (the last four follow
  `$XDG_CONFIG_HOME` instead of `~/.config` when that variable is set to an
  absolute path, which is where those tools look). Each of these, when present on the
  host, is covered by a tmpfs, so a sandboxed command sees it as an empty
  directory rather than reading your keys and tokens. That tmpfs is a normal
  writable filesystem owned by the sandboxed user, not a read-only view, so a
  command that writes into a masked directory succeeds and then loses the
  write when the sandbox exits: `ssh-keygen -f ~/.ssh/id_x`, `gh auth login`,
  and `aws configure` all exit `0` having silently discarded whatever they
  wrote, and because the exit status is zero, the hint that names a masked
  path on a failed command never fires for these. A read-only mask is
  possible (bwrap supports `--remount-ro`), but it would make any tool that
  writes into its own credential directory fail hard instead of silently
  losing the write, which is why the mask stays writable by default today.
  Use `sandbox-expose`
  (config key or repeatable `--sandbox-expose <path>` flag) to restore
  read-only access to a masked entry or a subpath of one when a command
  legitimately needs it; see `docs/CONFIG.md`. Three gaps remain: single-file
  credentials such as `~/.netrc` and `~/.npmrc` have no clean bwrap
  file-hiding primitive and are not masked; everything else under `$HOME`
  outside the nine directories above (`.env` files, browser profiles, shell
  history) stays fully readable; and live IPC credential endpoints are not
  touched by directory masking at all. The sandbox environment allowlist
  still forwards `DBUS_SESSION_BUS_ADDRESS` and `XDG_RUNTIME_DIR`, and `/run`
  stays readable through the read-only root bind, so the freedesktop secret
  service (gnome-keyring, KWallet) remains reachable from inside the sandbox,
  and tools built on it, such as `secret-tool`, `git-credential-libsecret`,
  `docker-credential-secretservice`, and the Python `keyring` package, can
  still read stored tokens. Masking directories does not address this, and it
  is not addressed by this change.
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
  under `/run/user`. The gpg-agent SSH socket is covered the same way as any
  other advertised agent: whatever socket `SSH_AUTH_SOCK` points to gets the
  `/dev/null` bind regardless of where it lives. The `~/.gnupg` directory
  mask additionally hides a gpg-agent socket when the socket file itself sits
  inside `~/.gnupg`, which is not the case on mainstream systemd
  distributions, where GnuPG's socket directory is `/run/user/$UID/gnupg`
  instead (confirm with `gpgconf --list-dirs socketdir`). There is no
  in-sandbox switch to re-enable the agent; recovery paths are
  `sandbox-expose ~/.ssh` for direct key-file auth
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
| `bwrap` (default) | Linux only. The bubblewrap mounts and namespaces described above, with the host network left open unless `sandbox-network = false` unshares it, in which case each command gets a fresh namespace with only its own private loopback. The nine built-in credential directories are masked by default and the advertised ssh-agent socket is cut off; see above. |
| `zerobox` | macOS and Linux. Denies writes, network access, and environment variables by default, with per-domain network allowances. `sandbox-network` is a bwrap-only key and does not apply here: zerobox denies network access under its own policy regardless of it. zerostack invokes `zerobox --allow-write <cwd> -- <shell> -c <command>`, so the working directory is writable and the rest of the policy is whatever zerobox enforces. Credential masking does not apply here: zerobox exposes no mount-policy surface to inject it, and whether its own defaults limit reads under `$HOME` has not been verified. |
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

`sandbox-expose` and `sandbox-network` are neither contract: they are
modifiers that change what the sandbox does when it runs, and neither of them
switches the sandbox on. `sandbox-network = false` with `sandbox = false` is a
no-op, warned about once at startup; `sandbox-network = false` with
`sandbox = true` alone is best effort in the same way as everything else, since
a missing backend still runs the command bare and online. Pairing it with
`sandbox-required` is what makes "the bash tool cannot reach the host network"
a guarantee, and that sentence is deliberately about the bash tool rather than
about the session.

Neither setting changes the gaps listed above. `sandbox-required` guarantees
that the isolation is present, not that the isolation is complete.
