use crate::agent::builder::dynamic_tool;
use crate::agent::tools;
use crate::extras::subagents::prompt;
use crate::provider::AnyModel;
use rig::DynModel;
use rig::agent::{Agent, AgentBuilder};
use rig::operation::Completion;
use rig::tool::DynamicTool;

/// The memory tools a subagent is granted: read-only access ONLY
/// (`memory_read`, `memory_search`). `memory_write` and `memory_edit` are
/// deliberately absent, so a subagent can never mutate the user's memory. This
/// is the single place the subagent memory tool set is assembled, so the
/// `subagent_memory_tool_set_excludes_memory_edit` test can guard it directly
/// instead of re-listing the tools it expects.
#[cfg(feature = "memory")]
pub(crate) fn subagent_memory_tools() -> Vec<DynamicTool> {
    vec![
        dynamic_tool(crate::extras::memory::MemoryRead::new(None, None)),
        dynamic_tool(crate::extras::memory::MemorySearch::new(None, None)),
    ]
}

#[allow(clippy::too_many_arguments)]
fn build_explore_agent_inner(
    model: DynModel<Completion>,
    max_turns: usize,
    max_text_file_size: u64,
    max_read_lines: u64,
    max_grep_results: u64,
    max_find_results: u64,
    max_list_dir_entries: Option<u64>,
    // OpenRouter `provider.order` pin for `anthropic/*` (see `AnyClient::completion_model`).
    additional_params: Option<serde_json::Value>,
    #[cfg(feature = "archmd")] architecture: Option<&str>,
) -> Agent {
    let mut preamble = prompt::explore_prompt();

    #[cfg(feature = "archmd")]
    if let Some(arch) = architecture
        && !arch.is_empty()
    {
        preamble.push_str("\n\n");
        preamble.push_str(arch);
    }

    if let Some(s) = crate::session::storage::load_suffix() {
        preamble.push_str("\n\n---\n\n");
        preamble.push_str(&s);
    }

    let tools: Vec<DynamicTool> = vec![
        dynamic_tool(tools::ReadTool::new(
            None,
            None,
            Some(max_text_file_size),
            max_read_lines,
        )),
        dynamic_tool(tools::GrepTool::new(None, None, max_grep_results)),
        dynamic_tool(tools::FindFilesTool::new(None, None, max_find_results)),
        dynamic_tool(tools::ListDirTool::new(None, None, max_list_dir_entries)),
    ];
    #[cfg(feature = "memory")]
    let mut tools = {
        let mut tools = tools;
        tools.extend(subagent_memory_tools());
        tools
    };
    #[cfg(not(feature = "memory"))]
    let mut tools = tools;

    // Respect --tools allowlist (intersection, no warnings for valid-but-not-applicable tools)
    {
        let allowlist = crate::extras::subagents::tools_allowlist_or_default();
        let cleaned: Vec<String> = allowlist
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !cleaned.is_empty() {
            let allowed: std::collections::HashSet<String> = cleaned.into_iter().collect();
            tools.retain(|t| allowed.contains(t.name()));
        }
    }

    #[cfg(feature = "hooks")]
    let tools = crate::extras::hooks::wrap_from_global(tools, None);

    let mut builder = AgentBuilder::new(model)
        .preamble(&preamble)
        .default_max_turns(max_turns);

    if let Some(params) = additional_params {
        builder = builder.additional_params(params);
    }

    let mut server = rig::tool::server::ToolServer::new();
    for tool in tools {
        server = server.dynamic_tool(tool);
    }
    builder.tool_server_handle(server.run()).build()
}

pub(crate) async fn build_explore_agent(
    model: AnyModel,
    max_turns: usize,
    cfg: &crate::config::Config,
    #[cfg(feature = "archmd")] architecture: Option<String>,
) -> Agent {
    let max_text_file_size = cfg.max_text_file_size.unwrap_or(10 * 1024 * 1024);
    let max_read_lines = cfg.resolve_subagent_max_read_lines();
    let max_grep_results = cfg.resolve_subagent_max_grep_results();
    let max_find_results = cfg.resolve_subagent_max_find_results();
    let max_list_dir_entries = cfg.resolve_subagent_max_list_dir_entries();
    #[cfg(feature = "archmd")]
    let arch_ref = architecture.as_deref();

    let (dyn_model, extra) = match model {
        AnyModel::OpenRouter(m) => (m.model, m.extra),
        other => (other.as_dyn(), None),
    };
    build_explore_agent_inner(
        dyn_model,
        max_turns,
        max_text_file_size,
        max_read_lines,
        max_grep_results,
        max_find_results,
        max_list_dir_entries,
        extra,
        #[cfg(feature = "archmd")]
        arch_ref,
    )
}
