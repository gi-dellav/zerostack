use std::sync::Arc;

use rig::tool::{DynamicTool, ToolContext, ToolExecutionError, ToolOutput};

use crate::permission::checker::PermCheck;

use super::dispatcher::HookDispatcher;
use super::{Decision, HookCtx, Verdict, session_context};

/// The only rig-typed file in the hook system (see design D1/D2): wraps a
/// `DynamicTool` so `PreToolUse`/`PostToolUseFailure` run around the inner call.
/// The wrapper is itself a `DynamicTool` whose closure runs the hooks and then
/// executes the inner tool inline with the same [`ToolContext`].
pub(crate) struct HookedTool {
    inner: DynamicTool,
    dispatcher: Arc<HookDispatcher>,
    permission: Option<PermCheck>,
}

/// Shared state behind a [`HookedTool`]'s dynamic closure.
struct HookedState {
    inner: DynamicTool,
    dispatcher: Arc<HookDispatcher>,
    permission: Option<PermCheck>,
}

impl HookedState {
    fn build_ctx(&self) -> HookCtx {
        let (session_id, session_path) = session_context();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let permission_mode = self
            .permission
            .as_ref()
            .map(|p| {
                p.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .mode()
                    .to_string()
            })
            .unwrap_or_else(|| "standard".to_string());
        HookCtx {
            session_id,
            session_path,
            cwd,
            permission_mode,
        }
    }
}

impl HookedTool {
    /// Wrap `inner` as a `DynamicTool` that runs the hook guard rail and then
    /// executes the inner tool inline with the same [`ToolContext`].
    fn into_dynamic(self) -> DynamicTool {
        let name = self.inner.name().to_string();
        let definition = self.inner.definition();
        let description = definition.description.clone();
        let parameters = definition.parameters.clone();
        let HookedTool {
            inner,
            dispatcher,
            permission,
        } = self;
        let this = Arc::new(HookedState {
            inner,
            dispatcher,
            permission,
        });
        DynamicTool::new_with_context(
            name,
            description,
            parameters,
            move |context: &mut ToolContext, args: serde_json::Value| {
                let this = this.clone();
                Box::pin(async move {
                    // Only lifecycle hooks are configured: no tool event can
                    // fire for this call, so skip building the per-call
                    // context (a `current_dir` syscall + permission lock) and
                    // run the inner tool directly.
                    if !this.dispatcher.has_tool_hooks() {
                        return this.inner.execute_with(context, args).await;
                    }
                    let tool_name = this.inner.name().to_string();
                    let ctx = this.build_ctx();
                    let tool_input = args.clone();
                    let args_str = args.to_string();

                    let pre = this
                        .dispatcher
                        .dispatch_pre_tool_use(&ctx, &tool_name, tool_input.clone())
                        .await;

                    match pre.verdict {
                        Verdict::Deny => {
                            let reason = pre.reason.unwrap_or_else(|| "denied by hook".to_string());
                            if let Some(perm) = &this.permission {
                                perm.lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .record_blocked(&tool_name, &args_str);
                            }
                            return Err(ToolExecutionError::other(format!(
                                "Blocked by guard rail: {reason}"
                            )));
                        }
                        // Forces the inner tool's own permission check to
                        // prompt regardless of mode; that check already
                        // escalates to deny in non-interactive contexts (no
                        // `ask_tx`), giving the spec's fail-closed behavior
                        // for free.
                        Verdict::Ask => {
                            if let Some(perm) = &this.permission {
                                perm.lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .force_ask_once(tool_name.clone());
                            }
                        }
                        // Suppresses the inner tool's own permission prompt
                        // for only this call; never bypasses a deny rule
                        // (checked first in `PermissionChecker::check` /
                        // `check_path`).
                        Verdict::Allow => {
                            if let Some(perm) = &this.permission {
                                perm.lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .allow_once(tool_name.clone());
                            }
                        }
                        Verdict::Defer => {}
                    }

                    // A PreToolUse hook may rewrite the arguments the inner
                    // tool actually runs with. Multiple rewrites are folded
                    // upstream in declared order; this applies the folded
                    // result.
                    let call_args = pre.updated_input.unwrap_or(args);

                    let result = this.inner.execute_with(context, call_args).await;

                    match &result {
                        Ok(output) => {
                            let response = output.render();
                            let decision = this
                                .dispatcher
                                .dispatch_post_tool_use(&ctx, &tool_name, tool_input, &response)
                                .await;
                            if let Decision::Rewrite { content } = decision {
                                return Ok(ToolOutput::text(content));
                            }
                        }
                        Err(e) => {
                            this.dispatcher
                                .dispatch_post_tool_use_failure(
                                    &ctx,
                                    &tool_name,
                                    tool_input,
                                    &e.to_string(),
                                )
                                .await;
                        }
                    }

                    result
                })
            },
        )
    }
}

/// Wraps every tool with the hook dispatcher's guard rail. Returns `tools`
/// unchanged when the dispatcher has no configured hooks (zero-cost
/// invariant).
pub(crate) fn wrap_all(
    tools: Vec<DynamicTool>,
    dispatcher: Arc<HookDispatcher>,
    permission: Option<PermCheck>,
) -> Vec<DynamicTool> {
    if dispatcher.is_empty() {
        return tools;
    }
    tools
        .into_iter()
        .map(|inner| {
            HookedTool {
                inner,
                dispatcher: dispatcher.clone(),
                permission: permission.clone(),
            }
            .into_dynamic()
        })
        .collect()
}
