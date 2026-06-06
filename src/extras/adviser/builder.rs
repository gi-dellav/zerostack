use crate::agent::tools;
use crate::extras::adviser::prompt;
use crate::provider::{AnyAgent, AnyModel, OpenAiAgent, OpenAiModel};
use rig::agent::{Agent, AgentBuilder};
use rig::completion::CompletionModel;

fn build_adviser_agent_inner<M: CompletionModel + 'static>(model: M, max_turns: usize) -> Agent<M> {
    let preamble = String::from(prompt::ADVISER_SYSTEM_PROMPT);

    let tools: Vec<Box<dyn rig::tool::ToolDyn>> = vec![
        Box::new(tools::ReadTool::new(None, None, Some(10 * 1024 * 1024))),
        Box::new(tools::GrepTool::new(None, None)),
        Box::new(tools::FindFilesTool::new(None, None)),
        Box::new(tools::ListDirTool::new(None, None)),
    ];

    AgentBuilder::new(model)
        .preamble(&preamble)
        .default_max_turns(max_turns)
        .tools(tools)
        .build()
}

pub(crate) async fn build_adviser_agent(model: AnyModel, max_turns: usize) -> AnyAgent {
    match model {
        AnyModel::OpenRouter(m) => AnyAgent::OpenRouter(build_adviser_agent_inner(m, max_turns)),
        AnyModel::OpenAI(m) => AnyAgent::OpenAI(match m {
            OpenAiModel::Responses(m) => {
                OpenAiAgent::Responses(build_adviser_agent_inner(m, max_turns))
            }
            OpenAiModel::Completions(m) => {
                OpenAiAgent::Completions(build_adviser_agent_inner(m, max_turns))
            }
        }),
        AnyModel::Anthropic(m) => AnyAgent::Anthropic(build_adviser_agent_inner(m, max_turns)),
        AnyModel::Gemini(m) => AnyAgent::Gemini(build_adviser_agent_inner(m, max_turns)),
        AnyModel::Ollama(m) => AnyAgent::Ollama(build_adviser_agent_inner(m, max_turns)),
    }
}
