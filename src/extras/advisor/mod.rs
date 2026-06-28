use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rig::completion::ToolDefinition;
use rig::streaming::StreamingChat;
use rig::tool::Tool;
use serde::Deserialize;
use tokio::sync::oneshot;

use crate::agent::tools::ToolError;
use crate::provider::{AnyClient, AnyModel, OpenAiModel};
use crate::retry::{self, RetryConfig};
use crate::session::{MessageRole, SessionMessage};

const ADVISOR_SYSTEM_PROMPT: &str = "\
You are an expert advisor called by a coding assistant for strategic guidance. \
The assistant is driving a real coding session with file read/write/edit, \
bash, grep, and other tools at its disposal.

Below is the full conversation so far, followed by the assistant's question. \
Your role:
- Review the conversation to understand what has happened
- Provide a clear plan, approach, or course correction
- Focus on architecture, design decisions, edge cases, and risk
- Keep guidance concise: aim for 150-300 words unless the question demands more
- Do NOT produce user-facing output or call any tools yourself

The assistant will continue the task after receiving your advice. \
Give it the strategic direction it needs to proceed correctly.";

pub struct HandoffRequest {
    pub question: String,
    pub reply: oneshot::Sender<String>,
}

pub type HandoffSender = tokio::sync::mpsc::Sender<HandoffRequest>;
pub type HandoffReceiver = tokio::sync::mpsc::Receiver<HandoffRequest>;

#[derive(Clone)]
pub struct AdvisorToolConfig {
    pub client: Option<AnyClient>,
    pub advisor_model: String,
    pub human_handoff: bool,
    pub max_uses: Option<usize>,
    pub handoff_tx: Option<HandoffSender>,
    pub enabled: bool,
    pub kilobytes_limit: u32,
}

static CONFIG: Mutex<Option<AdvisorToolConfig>> = Mutex::new(None);
static SESSION_MESSAGES: Mutex<Vec<SessionMessage>> = Mutex::new(Vec::new());

pub fn init_config(cfg: AdvisorToolConfig) {
    *CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = Some(cfg);
}

pub fn with_config<F, R>(f: F) -> R
where
    F: FnOnce(&AdvisorToolConfig) -> R,
{
    let guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = guard.as_ref().expect("advisor config not initialized");
    f(cfg)
}

pub fn update_client(client: AnyClient) {
    let mut guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mut cfg) = *guard {
        cfg.client = Some(client);
    }
}

pub fn set_session_messages(msgs: Vec<SessionMessage>) {
    *SESSION_MESSAGES.lock().unwrap_or_else(|e| e.into_inner()) = msgs;
}

#[derive(Deserialize)]
pub struct AdvisorArgs {
    pub question: String,
}

pub struct AdvisorTool {
    uses: AtomicUsize,
}

impl AdvisorTool {
    pub fn new() -> Self {
        Self {
            uses: AtomicUsize::new(0),
        }
    }
}

impl Tool for AdvisorTool {
    const NAME: &'static str = "advisor";
    type Error = ToolError;
    type Args = AdvisorArgs;
    type Output = String;

    async fn definition(&self, _p: String) -> ToolDefinition {
        let human_handoff = CONFIG
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|c| c.human_handoff))
            .unwrap_or(false);

        let desc = if human_handoff {
            "Consult the user for strategic guidance. \
Call this before substantive work, before writing, before committing to an \
interpretation, when stuck, or when considering a change of approach. \
The user sees your full conversation so far. \
Describe your question clearly — include relevant context, what you're \
trying to do, what you've tried, and what you need guidance on."
        } else {
            "Consult an expert advisor model for strategic guidance. \
The advisor receives your full conversation transcript automatically. \
Call this before substantive work, before writing, before committing to an \
interpretation, when stuck, or when considering a change of approach. \
Describe your question clearly — the advisor already sees the full \
conversation, so focus your question on the specific decision you need help with."
        };

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: desc.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "Your question for the advisor. The advisor \
            already sees the full conversation transcript. Focus on the specific decision, \
            approach, or problem you need guidance on."
                    }
                },
                "required": ["question"]
            }),
        }
    }

    async fn call(&self, args: AdvisorArgs) -> Result<String, ToolError> {
        if args.question.is_empty() {
            return Err(ToolError::Msg("advisor: question must not be empty".into()));
        }

        let cfg = with_config(|c| c.clone());

        if let Some(max) = cfg.max_uses {
            self.uses
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |u| {
                    if u >= max { None } else { Some(u + 1) }
                })
                .map_err(|_| {
                    ToolError::Msg("Advisor call limit reached for this request".into())
                })?;
        } else {
            self.uses.fetch_add(1, Ordering::Relaxed);
        }

        if cfg.human_handoff {
            let Some(ref tx) = cfg.handoff_tx else {
                return Err(ToolError::Msg(
                    "Human handoff unavailable (non-interactive mode)".into(),
                ));
            };
            let (reply_tx, reply_rx) = oneshot::channel();
            tx.send(HandoffRequest {
                question: args.question,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ToolError::Msg("Handoff channel closed".into()))?;

            match reply_rx.await {
                Ok(response) => {
                    if response.is_empty() {
                        Ok("[User provided no response]".to_string())
                    } else {
                        Ok(response)
                    }
                }
                Err(_) => Err(ToolError::Msg("Handoff cancelled".into())),
            }
        } else {
            let Some(ref client) = cfg.client else {
                return Err(ToolError::Msg("Advisor model not configured".into()));
            };

            let model = client.completion_model(cfg.advisor_model.clone());
            let messages = SESSION_MESSAGES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            run_advisor_completion(model, &args.question, &messages)
                .await
                .map_err(|e| ToolError::Msg(format!("Advisor call failed: {e}")))
        }
    }
}

async fn run_advisor_completion(
    model: AnyModel,
    question: &str,
    messages: &[SessionMessage],
) -> anyhow::Result<String> {
    let kilobytes_limit = with_config(|c| c.kilobytes_limit);
    let conversation = format_conversation(messages, kilobytes_limit);
    let prompt = format!(
        "## Conversation\n\n{}\n\n## Assistant's question\n\n{}",
        conversation, question
    );

    match model {
        AnyModel::OpenRouter(m, _) => advisor_call(m, prompt).await,
        AnyModel::OpenAI(m) => match m {
            OpenAiModel::Responses(m) => advisor_call(m, prompt).await,
            OpenAiModel::Completions(m) => advisor_call(m, prompt).await,
        },
        AnyModel::Anthropic(m) => advisor_call(m, prompt).await,
        AnyModel::Gemini(m) => advisor_call(m, prompt).await,
        AnyModel::Ollama(m) => advisor_call(m, prompt).await,
    }
}

pub(crate) fn format_conversation(msgs: &[SessionMessage], kilobytes_limit: u32) -> String {
    let per_side = (kilobytes_limit as usize * 1024) / 2;

    fn format_line(msg: &SessionMessage) -> String {
        let role = match msg.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
            MessageRole::ToolCall => "ToolCall",
            MessageRole::ToolResult => "ToolResult",
            MessageRole::SubagentToolCall => "SubagentToolCall",
        };
        format!("[{role}]: {}", msg.content)
    }

    // Collect head (oldest messages)
    let mut head_end = 0usize;
    let mut head_chars = 0usize;
    for (i, msg) in msgs.iter().enumerate() {
        let line = format_line(msg);
        let needed = if head_chars > 0 {
            line.len() + 2
        } else {
            line.len()
        };
        if head_chars + needed > per_side {
            break;
        }
        head_chars += needed;
        head_end = i + 1;
    }

    // Collect tail (newest messages)
    let mut tail_start = msgs.len();
    let mut tail_chars = 0usize;
    for (i, msg) in msgs.iter().enumerate().rev() {
        let line = format_line(msg);
        let needed = if tail_chars > 0 {
            line.len() + 2
        } else {
            line.len()
        };
        if tail_chars + needed > per_side {
            break;
        }
        tail_chars += needed;
        tail_start = i;
    }

    if msgs.is_empty() {
        return String::new();
    }

    let mut result = String::new();

    // Head
    for (i, msg) in msgs.iter().enumerate().take(head_end) {
        if i > 0 {
            result.push_str("\n\n");
        }
        result.push_str(&format_line(msg));
    }

    // Omission marker if there is a gap
    if head_end < tail_start {
        result.push_str("\n\n[... conversation omitted ...]\n\n");
    } else if head_end > 0 && head_end < msgs.len() {
        result.push_str("\n\n");
    }

    // Tail (only messages not already in head)
    let tail_begin = head_end.max(tail_start);
    for (i, msg) in msgs.iter().enumerate().skip(tail_begin) {
        if i > tail_begin {
            result.push_str("\n\n");
        }
        result.push_str(&format_line(msg));
    }

    result
}

async fn advisor_call<M>(model: M, prompt: String) -> anyhow::Result<String>
where
    M: rig::completion::CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
{
    let mut preamble = ADVISOR_SYSTEM_PROMPT.to_string();
    if let Some(s) = crate::session::storage::load_suffix() {
        preamble.push_str("\n\n---\n\n");
        preamble.push_str(&s);
    }

    let agent = rig::agent::AgentBuilder::new(model)
        .preamble(&preamble)
        .build();

    use futures::StreamExt;
    let history: Vec<rig::completion::Message> = vec![];
    let agent_ref = &agent;
    let mut stream = retry::retry_stream_chat(&RetryConfig::default(), move || {
        let p = prompt.clone();
        let h: Vec<rig::completion::Message> = vec![];
        async move { agent_ref.stream_chat(p, h).multi_turn(1).await }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Advisor call failed: {e}"))?;

    let mut response = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(rig::agent::MultiTurnStreamItem::FinalResponse(res)) => {
                response = res.response().to_string();
                break;
            }
            Err(e) => return Err(anyhow::anyhow!("Advisor call failed: {e}")),
            _ => {}
        }
    }

    if response.is_empty() {
        Ok("[Advisor returned empty response]".to_string())
    } else {
        Ok(response)
    }
}
