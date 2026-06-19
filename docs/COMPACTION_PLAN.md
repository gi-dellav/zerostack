# Mid-Turn Compaction — Design Notes

Status: implemented on branch `pr/mid-turn-compaction`. Phase 1 (observability) in commit `a32a622`; phase 2 (the config-gated trigger) follows. Captured 2026-06-01, revised 2026-06-14 after maintainer (gi-dellav) feedback on Matrix.

## Revision log

**2026-06-14 (gi-dellav, Matrix):**

- **Drop the soft limit.** Two thresholds (soft plus hard) are too complex. Keep a single hard limit only.
- **Make the hard limit a config setting, default off.** If the user does not set a value, mid-turn compaction does not run at all; the pre-existing between-turn behavior is unchanged. Setting a value opts in: compaction fires when real mid-turn prompt pressure crosses that percentage. Docs recommend **80%** as a reasonable starting value.
- **Keep the current summarization technique;** it is fine as is. The motivation for opting in at all is avoiding context rot in long contexts (see <https://garrit.xyz/posts/2026-05-06-dont-trust-large-context-windows>); a tighter, user-chosen ceiling keeps the live prompt small enough that the model still attends to it.

**2026-06-14 (implementation):**

- **`compact_enabled` is the master switch.** Confirmed not redundant: it gates the existing between-turn check (`event_handler.rs`, on `Done`, against the session text-token estimate) and now also gates mid-turn. When `compact_enabled = false`, nothing compacts. When `true`, mid-turn additionally fires if `mid_turn_compact_threshold` is a valid fraction in `(0.0, 1.0]`. The two levers are complementary: between-turn catches cross-turn history accumulation; mid-turn catches a single turn's in-flight tool traffic, which never enters the session estimate and so is invisible to the between-turn check.
- **Continuation prompt is a Rust `const`, not `prompts/auto-compact-continue.md`.** Every `.md` under `prompts/` is loaded as a user-selectable mode (`src/context/prompts.rs`), so a file there would pollute the `/prompt` picker. A `const MID_TURN_CONTINUE_PROMPT` in `src/ui/mod.rs` is truer to "hardcoded, no config override" and avoids the picker. Single template; the narrow-tool-calls urgency line is always included (any mid-turn fire means the ceiling was hit).
- **Clean abort boundary = the over-threshold `CompletionCall`.** At that point the model's just-returned tool calls have not executed yet, so aborting leaves no half-applied edits. The dominant pressure relief is that the respawn rebuilds history from the session (`convert_history`), dropping the aborted run's in-flight tool context; `handle_compress` is an additional step that no-ops when the session text history is itself under the limit.
- **Best-effort continuity.** Tool interactions live only in the runner and never reach the session, so before respawning we commit a recap built from `turn_trace` (a capped/truncated digest) plus any partial response text, as an Assistant message. This is lossy when a long turn overflows `turn_trace`'s cap; acceptable for v1, revisit if continuity proves weak.
- **Runaway guard / hard stop.** A single `awaiting_compaction_relief` flag is set when we compact+respawn. If the *next* provider call is still over the ceiling, compaction provably cannot free enough space (the irreducible floor — system prompt, tool schemas, kept-recent transcript, reserved response — exceeds the budget), so `stop_turn_context_exhausted` aborts the turn rather than looping compact→respawn→compact forever. It prints the full arithmetic (context window, ceiling tokens and %, post-compaction prompt tokens and %, overflow, `reserve_tokens`, `keep_recent_tokens`) and concrete options (raise `context_window`/KV cache, raise the threshold above the shown %, lower `keep_recent_tokens`/`reserve_tokens`, or use a bigger-context model). If instead the post-compaction call comes back under the ceiling, the flag clears and a later accumulation in the same turn may compact again — so the flag distinguishes "compaction failed" from "fresh growth," and only the former is fatal.

## Problem

On a local llama.cpp server with a fixed 32k KV cache, zerostack reliably grinds the context past the limit during a single user turn, causing llama.cpp to either error or silently set `truncated = 1` (which corrupts the conversation). Auto-compaction does exist but never gets a chance to fire mid-turn.

## Current behavior

- Compaction trigger lives in `src/ui/event_handler.rs:319-345`. It only runs when the agent emits `AgentEvent::Done`, i.e. after rig's whole multi-turn stream completes.
- `Done` comes from `MultiTurnStreamItem::FinalResponse` in `src/agent/runner.rs:107-123`. Everything before that — tool calls, tool results, reasoning, token deltas — flows through but does not trigger any pressure check.
- `session.total_estimated_tokens` (`src/session/mod.rs:65-67, 94-103`) is a local `len()/4` heuristic. It is updated by `add_message` only — which fires on user input and on `Done`, never on intermediate tool calls or results. Mid-turn the gauge is frozen.
- `session.total_input_tokens` and `total_output_tokens` exist and are populated from `usage` at `Done`, but they're cumulative billing counters, not "what's in the next prompt."
- Compaction is explicitly suppressed during `/loop` runs (`event_handler.rs:319`).

## The key discovery

rig already emits per-call usage. See `rig/crates/rig-core/src/agent/prompt_request/streaming.rs:45-66`:

```rust
pub enum MultiTurnStreamItem<R> {
    StreamAssistantItem(...),
    StreamUserItem(...),
    CompletionCall(CompletionCall),   // <-- this fires per provider call
    FinalResponse(FinalResponse),
}

pub struct CompletionCall {
    pub call_index: usize,
    pub usage: Option<Usage>,   // input_tokens / output_tokens
}
```

zerostack's `src/agent/runner.rs:131` has `_ => {}` that silently discards this event. The mid-turn blind spot is not a rig limitation — it's a missing match arm.

## Plan (ordered)

### 1. Tool output bounding at the tool boundary [START HERE]

The single biggest win and it requires no human in the loop. Cap large tool results before they enter rig's history.

- Touches: `src/fs.rs`, `src/sandbox.rs`, the subagent tool wrappers.
- Helper already exists: `src/extras/truncate.rs::truncate_cjk` — currently used only for subagent output. Extend / generalize.
- **Unit:** tool-native (lines for file/shell, matches for grep, entries for find). Real prompt-token impact is observed separately via rig's `CompletionCall` events — see option (2).
- **Cut shape:** head-only with a clear recovery hint. Simplest to implement, predictable behavior.
- **Defaults** (hardcoded; no config exposure yet — revisit once we have operational experience):
  - **File read:** first ~1000 lines, then `[truncated after 1000 lines — file is M lines total; re-call with a line range to see more]`.
  - **Shell exec:** first ~200 lines, then `[truncated after 200 lines — N more lines elided; re-run with a narrower invocation or pipe through `tail`]`. Caveat: head-only loses trailing output (final test result, stack-trace bottom). Acceptable for v1; revisit if it bites.
  - **Grep:** first ~50 matches, then `[truncated after 50 matches — M more matches total; narrow the pattern or restrict to a path]`.
  - **Find / list_files:** first ~200 entries, then `[truncated after 200 entries — N more; narrow the glob or path]`.
- The recovery hint is load-bearing — without it the agent retries the same call. Verify on the Qwen 3.6 model that the hints actually steer behavior.
- **Postponed:** an optional fast-model summarization variant above a second threshold (~5000 tokens) would be more powerful but adds a per-fire round-trip. Revisit only if structural truncation proves insufficient.

### 2. Auto-compact between rig iterations

Use the `CompletionCall` event to track real prompt-token pressure and intervene between iterations (not mid-iteration — that corrupts state).

- Add `AgentEvent::CompletionCall { input_tokens, output_tokens }` to `src/event.rs`.
- Wire it in `src/agent/runner.rs` by replacing the `_ => {}` arm.
- In `src/ui/event_handler.rs`, on receipt:
  - `session.total_estimated_tokens = max(current, input_tokens + output_tokens)` so the status bar's `x/y (z%)` is honest.
  - Evaluate compaction trigger.
- **Pressure metric:** `pressure = last_input_tokens / context_window`, where `last_input_tokens` comes from the most recent `CompletionCall.usage` (real provider count, not the `len/4` estimate). `context_window` is the zerostack config value (24576 in current setup), already chosen with margin below the real KV cache (32k).
- **Config-gated single hard limit** (per gi-dellav, 2026-06-14; no soft limit):
  - New optional config field, e.g. `mid_turn_compact_threshold: Option<f64>` in `src/config/mod.rs`, expressed as a fraction (0.0 to 1.0). Sits alongside the existing `context_window` / `reserve_tokens` / `keep_recent_tokens` / `compact_enabled` fields and follows the same `Option<T>` plus `resolve_*` convention.
  - **Default is unset (`None`) = off.** Unlike the other resolvers, this one must *not* substitute an enabling default: `resolve_mid_turn_compact_threshold(&self) -> Option<f64>` returns the option as-is. `None` means the mid-turn trigger never evaluates and behavior is exactly today's between-turn-only compaction.
  - When `Some(t)`, the trigger is simply:
    ```
    should_compact_now = pressure > t
    ```
    No iteration counter, no second threshold. The status-bar `max(estimate, real)` update from phase 1 still happens unconditionally; only the *trigger* is gated on the config value being set.
  - **Docs recommend 80%** (`0.80`) as a reasonable starting value. With `context_window = 24576` that is ~19.7k tokens, leaving headroom below the 32k KV cache. The recommendation goes in `docs/CONFIG.md`; the field stays unset in the default config so out-of-the-box behavior is unchanged.
  - Validation lives in the resolver: `resolve_mid_turn_compact_threshold` returns `Some(t)` only for `t` in `(0.0, 1.0]`, else `None`. A value of `0` would compact constantly, so `<= 0` (and `> 1`) silently behave as unset rather than wedging the agent.
- **As implemented:** the trigger is evaluated in the main UI event loop (`src/ui/mod.rs`) when an `AgentEvent::CompletionCall` arrives, gated on `is_running && !loop_running && !cli.no_session && resolve_compact_enabled() && context_window > 0 && resolve_mid_turn_compact_threshold().is_some()`. `pressure = input_tokens / context_window`. The clean abort boundary is the over-threshold `CompletionCall` itself (the model's just-returned tool calls have not executed, so no half-applied edits). `mid_turn_compact_and_respawn` then aborts the runner, records a progress recap, runs `handle_compress` on the session, and respawns via `spawn_runner(MID_TURN_CONTINUE_PROMPT, convert_history(session))`. Dropping the aborted run's in-flight tool context (absent from `convert_history`) is the dominant relief; `handle_compress` no-ops when the session text history is under its own limit.
- **Continuation prompt** is a Rust `const MID_TURN_CONTINUE_PROMPT` in `src/ui/mod.rs`, *not* a `prompts/*.md` file: every `.md` under `prompts/` loads as a selectable `/prompt` mode, which a continuation template should not be. Hardcoded, no config override. Single template (the soft/hard variants are gone with the soft limit):

  ```
  [Context was compacted to save space; the full prior history is in the
  system summary above.]

  Continue with the user's original task. Do not redo work already completed
  per the summary; focus on what remains. Context was tight, so prefer narrower
  follow-up tool calls over wide ones until pressure subsides.
  ```

  The acknowledgement that compaction happened is deliberate: it lets the agent read the summary as "what I did," not as part of the user's new instructions. The narrow-tool-calls line is always included, since any mid-turn fire means the user-chosen ceiling was hit, so the urgency always applies.

### 3. Subagent dispatch by default for context-hungry work

Prompt-level change only — zerostack already has the infrastructure: the `task` tool (`src/extras/subagents/task_tool.rs`, name = `"task"`) and a built-in `EXPLORE_PROMPT` (`src/extras/subagents/prompt.rs`) for a read-only investigator.

- The structural win is **fresh context**, not parallelism. A subagent that reads 20 files eats those 20 file reads in its own context and returns a 500-token summary; main agent's history grows by 500 tokens instead of 20,000.
- **Files to edit:** `prompts/default.md`, `prompts/code.md`, `prompts/debug.md`, `prompts/refactor.md`, `prompts/review.md`, `prompts/review-security.md`. Skip `ask.md` (low-touch), `plan.md` (already terse), and the non-coding modes (`brainstorm.md`, `simplify.md`, `write-prompt.md`, `autoconfig.md`, `frontend-design.md`).
- **Add a new section `## Subagent Dispatch`** (placed after `## Process`) with identical wording across all six prompts. **Hardened wording (v2)** after the v1 phrasing failed on Qwen 3.6 for an enumeration task ("List all test names in this project" — answered 285, actual 245, never reached for `task`):

  ```
  ## Subagent Dispatch

  Delegate to the `task` tool whenever the answer requires
  synthesizing across multiple search results. This includes:

  - **Enumeration:** "list / count / find ALL X across the
    codebase" — never assemble a count by adding up partial
    grep results yourself; the subagent verifies completeness.
  - **Cross-reference:** "where is X used", "how does Y work",
    "what calls Z" — anything touching multiple files.
  - **Investigation:** any question requiring more than one
    grep/read to answer.

  Reserve direct `read` / `grep` / `find_files` for known-
  location work: editing a specific file, reading one
  identified function, grepping for a literal you will act
  on immediately.

  **Anti-pattern:** running grep multiple times to find "all"
  matches and synthesizing a count is unreliable — truncation,
  overlapping regexes, and partial views all corrupt the
  answer. Use `task` instead.
  ```

- Strong directive ("Delegate", "never assemble"). Identical wording across all six files so behavior is predictable when the user switches modes.
- The `task` tool's own description ("Delegate a MULTI-STEP read-only investigation... NOT for single-step operations") backstops the prompt guidance. Qwen weighed that description as describing the *number of tool calls*, not the *scope of synthesis* — the v2 wording targets the latter directly.
- **v1 rationale lesson:** "anything touching ~3 files" let Qwen rationalize that a single `grep` call is one action regardless of internal file walks. v2 names the synthesis-from-partials failure mode explicitly so there's no semantic wiggle room.
- **v2 rationale lesson:** even the hardened mode-prompt wording didn't move Qwen's behavior — it could *recite* the guidance and *agree* it should have followed it, but its decision-time tool selection still went around. Cause: **three contradictory sources** of guidance, with the restrictive ones earlier in context and marked CRITICAL:
  1. `src/agent/prompt.rs::SYSTEM_PROMPT` line 16 (Read Operations section, CRITICAL): "Use the Task tool ONLY for specific multi-step investigations... Do NOT use it for single-step operations." — earliest, loudest.
  2. `src/agent/prompt.rs::SYSTEM_PROMPT` line 26 (tool inventory): "Use ONLY when answering needs several file reads... NOT for single operations."
  3. `src/extras/subagents/task_tool.rs::definition()` description: "Use ONLY when answering a question requires searching several files... Do NOT use for single-step operations."

  When CRITICAL-marked early-context guidance contradicts later mode-prompt guidance, the model picks the loudest. **v3 affordance change:** rewrite all three sources to invite `task` use ("Search and investigate via a fresh-context subagent. Use for any cross-file question..."), and remove the negative-gating "ONLY"/"NOT for" language entirely. Mode prompts (v2 wording) stay aligned.

- **Open question after v3:** if Qwen still doesn't reach for `task` on enumeration tasks despite all three sources being aligned and inviting, the next move is structural (option 2 mid-iteration auto-compact as backstop) rather than further prompt-engineering. Bound the prompt-tuning investment here.

#### Interaction with llama.cpp slots

Each slot is a separate KV cache; they share weights but not context memory.

- **1 slot:** Subagent handoffs blow away the cached prefix on every switch. Full prefill cost per handoff (seconds on 35B-A3B). Still wins on structural grounds because subagent context is fresh.
- **2 slots:** Main and subagent each hold their own warm KV. Handoffs cost nothing on the KV side.
- **VRAM cost:** 2×32k roughly doubles KV cache memory budget. On 16 GB with 35B Q8 already partially offloaded, going to 2 slots means either lower per-slot context (~24k) or more layers to CPU.

Recommendation: try 1 slot first with aggressive subagent dispatch. If prefill cost is painful, bump to 2 slots and accept per-slot context reduction.

## Deferred / Rejected

- **Soft limit (`iterations_since_last_compact >= K AND pressure > 0.65`).** Dropped 2026-06-14 per gi-dellav: two thresholds plus an iteration counter is more machinery than the problem warrants. A single user-set hard threshold is simpler to reason about and to document. If accumulating drag in long sessions turns out to matter, revisit, but only after the single-threshold version has operational time.
- **Preemptive compaction at lower thresholds.** Doesn't fix the mid-turn problem — only changes between-turn headroom. A single turn can still blow whatever headroom you set. Possible small refinement on top of (1)+(2), not a primary lever.
- **Tool-result deduplication.** Hash and dedupe identical tool results within a session. Real benefit (codebases re-read the same files constantly) but complex to implement correctly. Back burner.
- **Status-bar warnings / human-in-the-loop signals.** Defeats the purpose of agentic coding. Rejected.

## Hardware context

- 4070 Ti Super, 16 GB VRAM.
- Qwen 3.6 35B A3B MTP at Q8_0 (partial CPU offload).
- llama.cpp KV cache: 32k. zerostack `context_window`: 24576 (deliberate margin against the `len()/4` undercount).
- llama.cpp slots currently 1 (reduced from default 4 for VRAM). Willing to go to 2 if subagent dispatch warrants it.
- zerostack config: `reserve_tokens=8192`, `keep_recent_tokens=6000`, `max_tokens=6144`, `max_agent_turns=16`. Mid-turn compaction is opt-in via `mid_turn_compact_threshold` (unset by default); recommended starting value is `0.80`.

## Open questions to resolve before coding

_All resolved._
