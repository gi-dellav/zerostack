pub(crate) mod channel;
pub(crate) mod decorator;
pub(crate) mod dispatcher;
pub(crate) mod envelope;
pub(crate) mod normalize;
pub(crate) mod settings;
pub(crate) mod subprocess;
pub(crate) mod trust;

/// Outcome of a non-tool-permission hook dispatch: lifecycle events (`Stop`,
/// `UserPromptSubmit`, `SessionStart`, `SessionEnd`, `SubagentStart`,
/// `SubagentStop`) and `PostToolUse` result rewrite. Carries no rig type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// No hook expressed an opinion (or none matched); proceed unchanged.
    Continue,
    /// Block the action; `reason` is surfaced as user feedback or as the next
    /// instruction to the model.
    Block { reason: String },
    /// Replace model-visible content: injected context or a rewritten result.
    Rewrite { content: String },
}

/// Permission decision vocabulary for blockable tool events. Variants are
/// declared least to most severe so `Ord`/`max` picks the most severe of
/// several hook results, per the deterministic-merge requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    Allow,
    Defer,
    Ask,
    Deny,
}

/// Result of dispatching `PreToolUse`: the merged permission verdict plus any
/// input rewrite requested by a matching hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreDecision {
    pub verdict: Verdict,
    pub reason: Option<String>,
    pub updated_input: Option<serde_json::Value>,
}

/// Context common to every hook dispatch, used to assemble the stdin
/// envelope. `session_path` intentionally replaces Claude Code's
/// `transcript_path`. Carries no rig type.
#[derive(Debug, Clone)]
pub struct HookCtx {
    pub session_id: String,
    pub session_path: String,
    pub cwd: String,
    pub permission_mode: String,
}

/// `--loop` iteration/active state, threaded through the runner so the
/// `Stop` hook envelope can report `loop_iteration`/`loop_active`. `None`
/// outside loop mode (plain `-p` one-shot runs, or interactive sessions).
#[derive(Clone, Copy, Debug)]
pub struct LoopInfo {
    /// 1-based `--loop` iteration count.
    pub iteration: u32,
    /// Whether the loop will continue after this iteration.
    pub active: bool,
}

static PROCESS_SESSION_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Best-effort session identity for `HookCtx` until the session-lifecycle
/// seam supplies zerostack's real session id/path.
pub(crate) fn session_context() -> (String, String) {
    let session_id = PROCESS_SESSION_ID
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .clone();
    (session_id, String::new())
}

static DISPATCHER: std::sync::Mutex<Option<std::sync::Arc<dispatcher::HookDispatcher>>> =
    std::sync::Mutex::new(None);

/// Installs the process-wide `HookDispatcher`, built once from loaded and
/// trust-filtered config. Tool builders (`agent::builder`,
/// `extras::subagents::builder`) read it via [`get_dispatcher`].
pub(crate) fn init_dispatcher(dispatcher: dispatcher::HookDispatcher) {
    *DISPATCHER.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(dispatcher));
}

pub(crate) fn get_dispatcher() -> Option<std::sync::Arc<dispatcher::HookDispatcher>> {
    DISPATCHER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Wraps `tools` with the process-global dispatcher's guard rail, or returns
/// them unchanged when no hooks are configured. The single weave point shared
/// by the main-agent and subagent builders (design D5).
pub(crate) fn wrap_from_global(
    tools: Vec<Box<dyn rig::tool::ToolDyn>>,
    permission: Option<crate::permission::checker::PermCheck>,
) -> Vec<Box<dyn rig::tool::ToolDyn>> {
    match get_dispatcher() {
        Some(dispatcher) => decorator::wrap_all(tools, dispatcher, permission),
        None => tools,
    }
}

/// Outcome of gating a user prompt through `UserPromptSubmit` hooks.
pub(crate) enum PromptGate {
    /// The prompt (possibly with hook-injected context prepended) should be
    /// sent to the model.
    Proceed(String),
    /// A hook blocked the prompt; `String` is feedback to show the user
    /// instead of sending anything to the model.
    Blocked(String),
}

/// Testable core: dispatches `UserPromptSubmit` against an explicit
/// dispatcher/context and decides whether the prompt proceeds.
pub(crate) async fn gate_user_prompt(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    prompt: String,
) -> PromptGate {
    let decision = dispatcher
        .dispatch(
            "UserPromptSubmit",
            None,
            ctx,
            envelope::EventFields::UserPromptSubmit {
                prompt: prompt.clone(),
            },
        )
        .await;
    match decision {
        Decision::Continue => PromptGate::Proceed(prompt),
        Decision::Block { reason } => PromptGate::Blocked(reason),
        Decision::Rewrite { content } => PromptGate::Proceed(format!("{content}\n\n{prompt}")),
    }
}

/// Builds a `HookCtx` from process/session state for seams that don't have
/// live access to the `PermissionChecker` (`permission_mode` is a
/// placeholder; the safety-critical `PreToolUse`/`PostToolUse` path carries
/// the real mode via `HookedTool` instead).
pub(crate) fn best_effort_ctx() -> HookCtx {
    let (session_id, session_path) = session_context();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    HookCtx {
        session_id,
        session_path,
        cwd,
        permission_mode: "unknown".to_string(),
    }
}

/// Production entry point: reads the process-wide dispatcher and gates
/// `prompt` through `UserPromptSubmit` hooks. A prompt passes through
/// unchanged when no dispatcher is installed.
pub(crate) async fn dispatch_user_prompt_submit(prompt: String) -> PromptGate {
    let Some(dispatcher) = get_dispatcher() else {
        return PromptGate::Proceed(prompt);
    };
    gate_user_prompt(&dispatcher, &best_effort_ctx(), prompt).await
}

/// Outcome of dispatching `Stop`: either release (send the final response) or
/// force continuation with the hook's reason as the next instruction.
pub(crate) enum StopGate {
    Release,
    Continue { reason: String },
}

/// Testable core: dispatches `Stop` against an explicit dispatcher/context.
pub(crate) async fn gate_stop(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    stop_hook_active: bool,
    loop_iteration: Option<u64>,
    loop_active: Option<bool>,
) -> StopGate {
    let decision = dispatcher
        .dispatch(
            "Stop",
            None,
            ctx,
            envelope::EventFields::Stop {
                stop_hook_active,
                loop_iteration,
                loop_active,
            },
        )
        .await;
    match decision {
        Decision::Block { reason } => StopGate::Continue { reason },
        Decision::Continue | Decision::Rewrite { .. } => StopGate::Release,
    }
}

/// Production entry point: reads the process-wide dispatcher and gates
/// completion through `Stop` hooks. Releases immediately when no dispatcher
/// is installed.
pub(crate) async fn dispatch_stop(
    stop_hook_active: bool,
    loop_iteration: Option<u64>,
    loop_active: Option<bool>,
) -> StopGate {
    let Some(dispatcher) = get_dispatcher() else {
        return StopGate::Release;
    };
    gate_stop(
        &dispatcher,
        &best_effort_ctx(),
        stop_hook_active,
        loop_iteration,
        loop_active,
    )
    .await
}

/// Testable core: dispatches `SessionStart`. Informational only (not
/// blockable per the hook-lifecycle-events spec), so the decision is ignored.
pub(crate) async fn dispatch_session_start_with(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    source: &str,
) {
    let _ = dispatcher
        .dispatch(
            "SessionStart",
            None,
            ctx,
            envelope::EventFields::SessionStart {
                source: source.to_string(),
            },
        )
        .await;
}

/// Production entry point: reads the process-wide dispatcher and dispatches
/// `SessionStart` with `source` (one of `startup`, `resume`, `clear`; `compact`
/// is not emitted here). No-op when no dispatcher is installed.
pub(crate) async fn dispatch_session_start(source: &str) {
    let Some(dispatcher) = get_dispatcher() else {
        return;
    };
    dispatch_session_start_with(&dispatcher, &best_effort_ctx(), source).await;
}

/// Testable core: dispatches `SessionEnd`. Informational only (not
/// blockable per the hook-lifecycle-events spec), so the decision is ignored.
pub(crate) async fn dispatch_session_end_with(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    reason: &str,
) {
    let _ = dispatcher
        .dispatch(
            "SessionEnd",
            None,
            ctx,
            envelope::EventFields::SessionEnd {
                reason: reason.to_string(),
            },
        )
        .await;
}

/// Production entry point: reads the process-wide dispatcher and dispatches
/// `SessionEnd` with `reason`. No-op when no dispatcher is installed.
pub(crate) async fn dispatch_session_end(reason: &str) {
    let Some(dispatcher) = get_dispatcher() else {
        return;
    };
    dispatch_session_end_with(&dispatcher, &best_effort_ctx(), reason).await;
}

/// Testable core: dispatches `SubagentStart`, matching on the normalized agent
/// type. Returns hook-injected context to prepend to the child's prompt, if
/// any.
pub(crate) async fn gate_subagent_start(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    agent_type: &str,
) -> Option<String> {
    let canonical = normalize::canonical_tool_name(agent_type);
    let decision = dispatcher
        .dispatch(
            "SubagentStart",
            Some(&canonical),
            ctx,
            envelope::EventFields::SubagentStart {
                agent_type: canonical.clone(),
            },
        )
        .await;
    match decision {
        Decision::Rewrite { content } => Some(content),
        Decision::Continue | Decision::Block { .. } => None,
    }
}

/// Production entry point: reads the process-wide dispatcher and gates
/// subagent start for `agent_type`. Returns `None` when no dispatcher is
/// installed.
pub(crate) async fn dispatch_subagent_start(agent_type: &str) -> Option<String> {
    let dispatcher = get_dispatcher()?;
    gate_subagent_start(&dispatcher, &best_effort_ctx(), agent_type).await
}

/// Outcome of dispatching `SubagentStop`: release the child's result, or
/// force it to continue with the hook's reason as the next instruction.
pub(crate) enum SubagentStopGate {
    Release,
    Continue { reason: String },
}

/// Testable core: dispatches `SubagentStop`, matching on the normalized agent
/// type.
pub(crate) async fn gate_subagent_stop(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    agent_type: &str,
    stop_hook_active: bool,
) -> SubagentStopGate {
    let canonical = normalize::canonical_tool_name(agent_type);
    let decision = dispatcher
        .dispatch(
            "SubagentStop",
            Some(&canonical),
            ctx,
            envelope::EventFields::SubagentStop {
                stop_hook_active,
                agent_type: canonical.clone(),
            },
        )
        .await;
    match decision {
        Decision::Block { reason } => SubagentStopGate::Continue { reason },
        Decision::Continue | Decision::Rewrite { .. } => SubagentStopGate::Release,
    }
}

/// Production entry point: reads the process-wide dispatcher and gates
/// subagent completion for `agent_type`. Releases immediately when no
/// dispatcher is installed.
pub(crate) async fn dispatch_subagent_stop(
    agent_type: &str,
    stop_hook_active: bool,
) -> SubagentStopGate {
    let Some(dispatcher) = get_dispatcher() else {
        return SubagentStopGate::Release;
    };
    gate_subagent_stop(
        &dispatcher,
        &best_effort_ctx(),
        agent_type,
        stop_hook_active,
    )
    .await
}

/// Testable core of the `hooks test` dry-run: dispatches `PreToolUse` against
/// an explicit dispatcher/context and formats a human-readable report.
pub(crate) async fn hooks_test_dry_run_with(
    dispatcher: &dispatcher::HookDispatcher,
    ctx: &HookCtx,
    tool_name: &str,
    tool_input: serde_json::Value,
) -> String {
    let decision = dispatcher
        .dispatch_pre_tool_use(ctx, tool_name, tool_input)
        .await;
    format!(
        "hooks test PreToolUse tool={tool_name:?}\n  verdict: {:?}\n  reason: {}\n  updated_input: {}",
        decision.verdict,
        decision.reason.as_deref().unwrap_or("(none)"),
        decision
            .updated_input
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(none)".to_string()),
    )
}

/// Production entry point: reads the process-wide dispatcher and dry-runs
/// `PreToolUse` for `tool_name`/`tool_input`, for the `hooks test` CLI flag.
pub(crate) async fn hooks_test_dry_run(tool_name: &str, tool_input: serde_json::Value) -> String {
    let Some(dispatcher) = get_dispatcher() else {
        return "hooks: no dispatcher installed (--no-hooks, disableAllHooks, or no settings.json hooks found)".to_string();
    };
    hooks_test_dry_run_with(&dispatcher, &best_effort_ctx(), tool_name, tool_input).await
}
