use std::time::Duration;

use futures::future::join_all;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::tools::{ToolError, check_perm};
use crate::extras::subagents::builder;
use crate::extras::subagents::{take_subagent_event_tx, with_config};
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

/// Per-subagent wall-clock timeout. If a subagent doesn't finish within this
/// window its output is replaced with a timeout marker and the remaining tasks
/// continue independently.
const SUBAGENT_TIMEOUT: Duration = Duration::from_secs(300);

/// Hard cap on a single subagent's response, protecting the main agent's
/// context window from a runaway multi-megabyte exploration result.
const MAX_SUBAGENT_RESPONSE_BYTES: usize = 128 * 1024;

#[derive(Deserialize)]
pub struct TaskArgs {
    /// One or more exploration prompts. When multiple are provided,
    /// they are explored in parallel subagents and results are combined.
    pub prompts: Vec<String>,
}

pub struct TaskTool {
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
}

impl TaskTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        Self { permission, ask_tx }
    }
}

impl Tool for TaskTool {
    const NAME: &'static str = "task";
    type Error = ToolError;
    type Args = TaskArgs;
    type Output = String;

    async fn definition(&self, _p: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Delegate read-only codebase exploration to a subagent. \
Provide one or more prompts describing what to investigate. \
Multiple prompts are explored in parallel. \
The subagent can read, grep, find_files, list directories, and access memory. \
Returns a summary of findings for each prompt."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One or more exploration tasks (parallel when multiple)"
                    }
                },
                "required": ["prompts"]
            }),
        }
    }

    async fn call(&self, args: TaskArgs) -> Result<String, ToolError> {
        if args.prompts.is_empty() {
            return Err(ToolError::Msg("task: prompts must not be empty".into()));
        }

        check_perm(
            &self.permission,
            &self.ask_tx,
            Self::NAME,
            &args.prompts.join(" | "),
        )
        .await?;

        let (client, model_name, max_turns) =
            with_config(|cfg| (cfg.client.clone(), cfg.model_name.clone(), cfg.max_turns));

        let subagent_event_tx = take_subagent_event_tx();

        // Spawn one task per prompt, each guarded by a wall-clock timeout.
        // AbortHandles are stored in a guard so that if the parent future is
        // dropped (user cancels, session exits) all in-flight subagents are
        // aborted rather than leaking.
        let mut abort_handles: Vec<tokio::task::AbortHandle> = Vec::new();
        let mut handles = Vec::with_capacity(args.prompts.len());
        for (i, prompt_text) in args.prompts.iter().enumerate() {
            let prompt_text = prompt_text.clone();
            let model = client.completion_model(model_name.clone());
            let event_tx = subagent_event_tx.clone();
            let join_handle = tokio::spawn(async move {
                let work = async {
                    let architecture = with_config(|cfg| {
                        #[cfg(feature = "archmd")]
                        {
                            cfg.architecture.clone()
                        }
                        #[cfg(not(feature = "archmd"))]
                        {
                            None::<String>
                        }
                    });
                    let agent = builder::build_explore_agent(model, max_turns, architecture).await;
                    agent
                        .run_subagent(&prompt_text, max_turns, event_tx.as_ref())
                        .await
                };
                match tokio::time::timeout(SUBAGENT_TIMEOUT, work).await {
                    Ok(Ok(response)) => (i, prompt_text, Ok(response)),
                    Ok(Err(e)) => (i, prompt_text, Err(format!("[error: {}]", e))),
                    Err(_elapsed) => (
                        i,
                        prompt_text,
                        Err("[timeout: subagent exceeded 300s]".to_string()),
                    ),
                }
            });
            abort_handles.push(join_handle.abort_handle());
            handles.push(join_handle);
        }

        // Abort guard — if this future is dropped, all subagents are cancelled.
        let _guard = SubagentGuard {
            handles: abort_handles,
        };

        let results = join_all(handles).await;

        let mut outputs: Vec<(usize, String, String)> = Vec::new();
        for r in results {
            match r {
                Ok((i, prompt_text, Ok(response))) => {
                    outputs.push((i, prompt_text, truncate_response(&response)));
                }
                Ok((i, prompt_text, Err(e))) => {
                    outputs.push((i, prompt_text, e));
                }
                Err(e) => {
                    outputs.push((
                        outputs.len(),
                        "(unknown)".to_string(),
                        format!("[task panicked: {}]", e),
                    ));
                }
            }
        }

        outputs.sort_by_key(|(i, _, _)| *i);

        Ok(combine_results(&outputs))
    }
}

/// Truncate a subagent response to at most `max` bytes on a UTF-8 char boundary,
/// appending a marker so the parent agent knows the result was capped.
pub(crate) fn truncate_response(s: &str) -> String {
    let max = MAX_SUBAGENT_RESPONSE_BYTES;
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = s[..cut].to_string();
    out.push_str(&format!(
        "\n…[subagent response truncated at {}B]",
        MAX_SUBAGENT_RESPONSE_BYTES
    ));
    out
}

/// Combine per-task outputs into a single Markdown string, ordered by the
/// original prompt index. Multiple tasks get `## Task N:` headings; a single
/// task is emitted as-is.
pub(crate) fn combine_results(outputs: &[(usize, String, String)]) -> String {
    let mut combined = String::new();
    for (idx, (_, prompt_text, response)) in outputs.iter().enumerate() {
        if outputs.len() > 1 {
            if idx > 0 {
                combined.push('\n');
            }
            let label = prompt_text.chars().take(60).collect::<String>();
            combined.push_str(&format!("## Task {}: {}\n\n", idx + 1, label));
        }
        combined.push_str(response);
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
    }
    combined
}

/// Aborts all registered subagent tasks on drop. If the parent agent cancels
/// the `task` tool call (e.g. the session ends or the loop exits), in-flight
/// subagents are stopped immediately rather than leaking.
struct SubagentGuard {
    handles: Vec<tokio::task::AbortHandle>,
}

impl Drop for SubagentGuard {
    fn drop(&mut self) {
        for h in &self.handles {
            h.abort();
        }
    }
}
