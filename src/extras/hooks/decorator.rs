use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;

use crate::agent::tools::ToolError as LocalToolError;
use crate::permission::checker::PermCheck;

use super::dispatcher::HookDispatcher;
use super::{Decision, HookCtx, Verdict, session_context};

/// The only rig-typed file in the hook system (see design D1/D2): wraps a
/// `ToolDyn` so `PreToolUse`/`PostToolUseFailure` run around the inner call.
/// Overrides `call` only, per rig 0.39's `ToolDyn` surface.
pub(crate) struct HookedTool {
    inner: Box<dyn ToolDyn>,
    dispatcher: Arc<HookDispatcher>,
    permission: Option<PermCheck>,
}

impl HookedTool {
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

impl ToolDyn for HookedTool {
    fn name(&self) -> String {
        self.inner.name()
    }

    // `WasmBoxedFuture` is the return type rig's `ToolDyn` trait requires for
    // `definition`/`call`. On native targets (this crate never builds for
    // wasm32) it is a plain `Pin<Box<dyn Future + Send>>`; rig only drops the
    // `Send` bound on wasm32.
    fn definition<'a>(&'a self, prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        self.inner.definition(prompt)
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            // Only lifecycle hooks are configured: no tool event can fire for
            // this call, so skip building the per-call context (a `current_dir`
            // syscall + permission lock) and run the inner tool directly.
            if !self.dispatcher.has_tool_hooks() {
                return self.inner.call(args).await;
            }
            let tool_name = self.inner.name();
            let ctx = self.build_ctx();
            let tool_input: serde_json::Value =
                serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);

            let pre = self
                .dispatcher
                .dispatch_pre_tool_use(&ctx, &tool_name, tool_input.clone())
                .await;

            match pre.verdict {
                Verdict::Deny => {
                    let reason = pre.reason.unwrap_or_else(|| "denied by hook".to_string());
                    if let Some(perm) = &self.permission {
                        perm.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .record_blocked(&tool_name, &args);
                    }
                    return Err(ToolError::ToolCallError(Box::new(LocalToolError::Msg(
                        format!("Blocked by guard rail: {reason}"),
                    ))));
                }
                // Forces the inner tool's own permission check to prompt
                // regardless of mode; that check already escalates to deny
                // in non-interactive contexts (no `ask_tx`), giving the
                // spec's fail-closed behavior for free.
                Verdict::Ask => {
                    if let Some(perm) = &self.permission {
                        perm.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .force_ask_once(tool_name.clone());
                    }
                }
                // Suppresses the inner tool's own permission prompt for only
                // this call; never bypasses a deny rule (checked first in
                // `PermissionChecker::check`/`check_path`).
                Verdict::Allow => {
                    if let Some(perm) = &self.permission {
                        perm.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .allow_once(tool_name.clone());
                    }
                }
                Verdict::Defer => {}
            }

            // A PreToolUse hook may rewrite the arguments the inner tool
            // actually runs with. Multiple rewrites are folded upstream in
            // declared order; this applies the folded result.
            let call_args = match &pre.updated_input {
                Some(rewritten) => serde_json::to_string(rewritten).unwrap_or(args),
                None => args,
            };

            let result = self.inner.call(call_args).await;

            match &result {
                Ok(response) => {
                    let decision = self
                        .dispatcher
                        .dispatch_post_tool_use(&ctx, &tool_name, tool_input, response)
                        .await;
                    if let Decision::Rewrite { content } = decision {
                        return Ok(content);
                    }
                }
                Err(e) => {
                    self.dispatcher
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
    }
}

/// Wraps every tool with the hook dispatcher's guard rail. Returns `tools`
/// unchanged when the dispatcher has no configured hooks (zero-cost
/// invariant).
pub(crate) fn wrap_all(
    tools: Vec<Box<dyn ToolDyn>>,
    dispatcher: Arc<HookDispatcher>,
    permission: Option<PermCheck>,
) -> Vec<Box<dyn ToolDyn>> {
    if dispatcher.is_empty() {
        return tools;
    }
    tools
        .into_iter()
        .map(|inner| {
            Box::new(HookedTool {
                inner,
                dispatcher: dispatcher.clone(),
                permission: permission.clone(),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}
