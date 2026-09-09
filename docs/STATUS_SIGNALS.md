---
description: "Wire protocol for the status-signal Unix domain socket: every message, its ordering guarantees, the run-boundary invariant, listener rules, and which modes emit what."
---

# Status signals

This is the single source of truth for the status-signal protocol. `docs/CONFIG.md` and
`README.md` link here instead of restating the message list; if you are editing the
protocol, this is the only file that needs to change.

## Overview

zerostack can report its run state to an external supervisor (a status bar, a
parallel-agent manager, or any other tooling) over a Unix domain socket. The
direction is one-way: zerostack is the sender, the listener is a passive
observer. Delivery is best effort: zerostack connects, writes, and drops the
connection for every message; it never blocks its own run waiting for the
listener to be there, and it never retries or queues a message the listener
missed.

## Enabling

The `status-signals` feature is included in the default build, so no extra
build flags are needed. Pass `--status-socket <path>` on the command line to
turn emission on; with the flag omitted, zerostack sends nothing.

The listener must bind and `accept()` the Unix domain socket at `<path>`
before zerostack starts sending. Each message is a fresh connection: if
nothing is listening when zerostack connects, the write is silently dropped
and the run continues exactly as it would without `--status-socket`.

## Message table

Every message is one ASCII line terminated by a single `\n`, delivered as its
own connect-write-close.

| Message              | Since | Meaning |
| --------------------- | ----- | ------- |
| `start`               | v1.0  | A turn has begun. |
| `stop`                | v1.0  | A turn has ended. |
| `git-conflict`        | v1.0  | A worktree merge hit a conflict and is prompting the user. Emitted at two worktree-merge prompts; there is currently no matching "left this wait" signal. |
| `blocked:permission`  | v1.1  | The interactive permission prompt is on screen and zerostack is waiting for a human decision. |
| `state:working`       | v1.1  | A wait reported by a `blocked:<reason>` message has ended and zerostack is working again. |

`blocked:<reason>` and `state:<state>` are lowercase ASCII tokens with no
whitespace and no `\n`. This protocol version defines exactly one reason,
`permission`, and exactly one state, `working`. Further reasons and states
(for example a `/chain` wait, a headless plan-reuse wait, or an `idle` state)
are reserved for later protocol versions and are not emitted today.

## Ordering guarantees

A permission wait is bracketed in this exact order:

```
start -> blocked:permission -> state:working -> ... -> stop
```

`blocked:permission` is sent only after the prompt is visible on screen, and
`state:working` is sent only after the user's decision has been taken, before
that decision is handed back to the waiting tool. This holds for every
decision the prompt accepts: allow once, allow always, deny, and Esc. If the
prompt path fails with an error after reporting `blocked:permission`, the
listener still receives `state:working` before the prompt path returns, so
the pair is always balanced.

`state:<state>` is a level-triggered report of the state zerostack is in
after a wait ends, not an event with pairing or nesting semantics. Waits are
serialised today (one prompt at a time), and a consumer must not build a
stack on `blocked:` / `state:` pairs. `git-conflict` stays a v1.0 attention
event without a release signal in this version.

## Run-boundary invariant

A `blocked:<reason>` or `state:<state>` message never adds an extra `start`
or `stop`, and never removes one. Across a whole turn, the subsequence made
of only `start`, `stop`, and `git-conflict` messages is byte-for-byte
identical, in the same relative order, to what protocol v1.0 emitted for that
turn. In other words: filter out every line that is not `start`, `stop`, or
`git-conflict`, and what remains is exactly the v1.0 stream.

## Listener rules

- Split incoming bytes on `\n`; a message is everything up to and including
  each `\n`.
- Ignore any line you do not recognise. This is how the protocol stays
  backward compatible: a v1.0 listener that ignores `blocked:*` and
  `state:*` sees no behaviour change when talking to a v1.1 sender.
- Every message is its own connection carrying exactly one line, so a read on
  an accepted stream yields at most one line; batching is not part of v1.1.
- Unlink a stale socket file before bind: a crashed listener leaves the inode
  behind, and every later signal is then silently dropped with
  `ECONNREFUSED`.
- The sender connects per message and blocks synchronously on `connect` and
  `write_all`, ignoring the result. Accept and read promptly: a listener that
  stalls in its accept loop stalls zerostack's own run, because the sender is
  waiting on that same connect/write to complete.
- If you track a single "current state" for display purposes, let the last
  recognised `state:<state>` win. A `state:working` that arrives after a
  `stop` (possible on some error paths) is harmless: `stop` still arrives and
  wins over it as the run boundary, and showing `working` for slightly
  longer than the run actually lasted is preferable to showing nothing.

## Trust model

zerostack connects to whatever path `--status-socket` names, never creates
it, and never checks the peer. Anything that can bind that path first
observes the agent's activity timeline, and, because the sender blocks on
`connect` and `write` with no timeout, can stall the agent (an accepted
trade-off of the flag, documented rather than bounded in this protocol
version). Put the socket in `$XDG_RUNTIME_DIR` or a 0700 directory, never a
shared `/tmp`. Messages intentionally carry no session content, no tool
names, no commands, no prompt text, and future reasons must keep that
property.

## Per-mode matrix

| Mode                | Emits v1.0 messages (`start`/`stop`/`git-conflict`) | Emits `blocked:permission` / `state:working` |
| -------------------- | ---------------------------------------------------- | ---------------------------------------------- |
| Interactive TUI      | Yes                                                   | Yes |
| Headless `-p`        | Yes                                                   | No |
| Headless `--loop`    | Yes                                                   | No |

Non-interactive modes (`-p` and `--loop`) pass no ask channel to the
permission checker, so an Ask verdict is denied before any prompt is ever
drawn. There is no permission wait to report, so these modes never emit
`blocked:permission` or the paired `state:working`; a consumer running
zerostack headlessly should not wait for either line.

## Version history

| Protocol version | First shipped in           | Messages added |
| ----------------- | --------------------------- | --------------- |
| v1.0               | zerostack v1.5.0             | `start`, `stop`, `git-conflict` |
| v1.1               | unreleased                   | `blocked:permission`, `state:working` |
