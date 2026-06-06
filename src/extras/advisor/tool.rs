use std::time::Duration;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::tools::{ToolError, check_perm};
use crate::extras::advisor::builder;
use crate::extras::advisor::{clone_advisor_event_tx, get_session_snapshot, with_config};
use crate::extras::truncate::truncate_cjk;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

const ADVISOR_TIMEOUT: Duration = Duration::from_secs(300);

const MAX_ADVISOR_RESPONSE_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
pub struct AdvisorArgs {
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Clone)]
pub struct AdvisorTool {
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
}

impl AdvisorTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        Self { permission, ask_tx }
    }
}

impl Tool for AdvisorTool {
    const NAME: &'static str = "advisor";
    type Error = ToolError;
    type Args = AdvisorArgs;
    type Output = String;

    async fn definition(&self, _p: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Consult a stronger model for strategic guidance. \
Call BEFORE committing to a complex approach, when stuck on a recurring error, \
when considering a change of approach, or when the task appears complete. \
The advisor sees the full conversation history and returns a plan, correction, \
or stop signal. Use BEFORE your own plan-level tools (like write_todo_list). \
On tasks longer than a few steps, call at least once before committing to an \
approach and once before declaring done. \
Takes an optional `query` to focus the advice on a specific question. \
If absent, the advisor infers what you need from context."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional specific question to focus the advisor on. If absent, the advisor infers what's needed from the conversation context."
                    }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: AdvisorArgs) -> Result<String, ToolError> {
        check_perm(
            &self.permission,
            &self.ask_tx,
            Self::NAME,
            args.query.as_deref().unwrap_or("(no query)"),
        )
        .await?;

        let (client, model_name, max_turns) =
            with_config(|cfg| (cfg.client.clone(), cfg.model_name.clone(), cfg.max_turns));

        let advisor_event_tx = clone_advisor_event_tx();

        let messages = get_session_snapshot();

        let mut conversation = String::from("## Conversation so far\n\n");

        // Limit to the most recent ~8000 chars of messages to avoid
        // overloading the advisor's context window while keeping enough
        // context for useful advice.
        let max_ctx_chars = 8000usize;
        let mut total = 0usize;
        let mut start_idx = messages.len();
        for (i, msg) in messages.iter().enumerate().rev() {
            let len = msg.content.len();
            if total + len > max_ctx_chars {
                start_idx = i + 1;
                break;
            }
            total += len;
            start_idx = i;
        }

        if start_idx > 0 {
            conversation.push_str(&format!(
                "(... {} earlier messages omitted for brevity)\n\n",
                start_idx
            ));
        }

        for msg in &messages[start_idx..] {
            match msg.role {
                crate::session::MessageRole::User => {
                    conversation.push_str("**User:** ");
                    conversation.push_str(&msg.content);
                    conversation.push_str("\n\n");
                }
                crate::session::MessageRole::Assistant => {
                    // Truncate very long assistant messages to avoid
                    // blowing up the advisor context
                    let content = &msg.content;
                    if content.len() > 2000 {
                        let preview: String = content.chars().take(2000).collect();
                        conversation.push_str(&format!(
                            "**Assistant:** {}\n*(message truncated at 2000 chars, {} total)*\n\n",
                            preview,
                            content.len()
                        ));
                    } else {
                        conversation.push_str("**Assistant: ");
                        conversation.push_str(content);
                        conversation.push_str("\n\n");
                    }
                }
                crate::session::MessageRole::System => {
                    conversation.push_str("**System:** ");
                    conversation.push_str(&msg.content);
                    conversation.push_str("\n\n");
                }
            }
        }

        if let Some(ref query) = args.query {
            conversation.push_str(&format!("## The agent's question\n\n{}\n", query));
        } else {
            conversation.push_str(
                "## The agent's request\n\nThe agent has consulted you for strategic guidance. \
                Based on the conversation above, provide your best advice on what the agent \
                should do next. Consider: is the current approach sound? Are there better \
                alternatives? Are there bugs or edge cases to watch for? What are the specific \
                next steps?\n",
            );
        }

        let model = client.completion_model(model_name.clone());

        if let Some(tx) = &advisor_event_tx {
            let _ = tx.send(crate::event::AgentEvent::AdvisorConsulting).await;
        }

        let work = async {
            let agent = builder::build_advisor_agent(model, max_turns).await;
            agent
                .run_subagent(&conversation, max_turns, advisor_event_tx.as_ref())
                .await
        };

        let result = match tokio::time::timeout(ADVISOR_TIMEOUT, work).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                if let Some(tx) = &advisor_event_tx {
                    let _ = tx
                        .send(crate::event::AgentEvent::AdvisorError {
                            error: compact_str::CompactString::new(format!("error: {}", e)),
                        })
                        .await;
                }
                return Err(ToolError::Msg(format!("advisor error: {}", e)));
            }
            Err(_elapsed) => {
                if let Some(tx) = &advisor_event_tx {
                    let _ = tx
                        .send(crate::event::AgentEvent::AdvisorError {
                            error: compact_str::CompactString::new(
                                "timeout: advisor exceeded 300s",
                            ),
                        })
                        .await;
                }
                return Err(ToolError::Msg(
                    "advisor timeout: advisor exceeded 300s".to_string(),
                ));
            }
        };

        let truncated = truncate_cjk(
            &result,
            MAX_ADVISOR_RESPONSE_BYTES,
            &format!(
                "\n…[advisor response truncated at {}B]",
                MAX_ADVISOR_RESPONSE_BYTES
            ),
        );

        if let Some(tx) = &advisor_event_tx {
            let _ = tx
                .send(crate::event::AgentEvent::AdvisorResult {
                    text: compact_str::CompactString::new(&truncated),
                })
                .await;
        }

        Ok(truncated)
    }
}
