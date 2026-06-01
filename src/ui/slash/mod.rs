mod add;
mod content;
mod features;
mod help;
mod init;
mod memory;
mod providers;
mod session;
mod settings;

pub(crate) use providers::warm_model_cache;

use smallvec::SmallVec;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::events::render_session;
use crate::ui::input::InputEditor;
use crate::ui::renderer::Renderer;

pub(crate) const C_AGENT: crossterm::style::Color = crossterm::style::Color::White;
pub(crate) const C_RESULT: crossterm::style::Color = crossterm::style::Color::DarkGrey;
pub(crate) const C_ERROR: crossterm::style::Color = crossterm::style::Color::Red;

pub struct SlashCtx<'a> {
    pub agent: &'a mut Option<AnyAgent>,
    pub client: &'a mut AnyClient,
    pub renderer: &'a mut Renderer,
    pub session: &'a mut Session,
    pub cli: &'a Cli,
    pub cfg: &'a Config,
    pub context: &'a mut ContextFiles,
    pub show_reasoning: &'a mut bool,
    pub reasoning_enabled: &'a mut bool,
    pub is_running: &'a mut bool,
    pub input: &'a mut InputEditor,
    pub permission: &'a Option<PermCheck>,
    pub ask_tx: &'a Option<AskSender>,
    pub todo_tools_enabled: &'a mut bool,
    pub sandbox: &'a Sandbox,
    #[cfg(feature = "loop")]
    pub loop_state: &'a mut Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "mcp")]
    pub mcp_manager: Option<&'a crate::extras::mcp::McpClientManager>,
}

impl SlashCtx<'_> {
    pub async fn rebuild_agent(&mut self) {
        let model = self.client.completion_model(self.session.model.to_string());
        *self.agent = Some(
            crate::provider::build_agent(
                model,
                self.cli,
                self.cfg,
                self.context,
                self.permission.clone(),
                self.ask_tx.clone(),
                self.sandbox.clone(),
                *self.reasoning_enabled,
                #[cfg(feature = "mcp")]
                self.mcp_manager,
            )
            .await,
        );
    }

    pub async fn rebuild_agent_with_client(
        &mut self,
        provider: &str,
        new_reasoning: bool,
    ) -> Result<(), anyhow::Error> {
        *self.client = crate::provider::create_client(
            provider,
            self.cli.api_key.as_deref(),
            &self.cfg.custom_providers_map(),
            self.cfg.api_keys.as_ref(),
        )?;
        let model = self.client.completion_model(self.session.model.to_string());
        *self.agent = Some(
            crate::provider::build_agent(
                model,
                self.cli,
                self.cfg,
                self.context,
                self.permission.clone(),
                self.ask_tx.clone(),
                self.sandbox.clone(),
                new_reasoning,
                #[cfg(feature = "mcp")]
                self.mcp_manager,
            )
            .await,
        );
        Ok(())
    }
}

pub(crate) fn write_ok(renderer: &mut Renderer, msg: impl std::fmt::Display) {
    let _ = renderer.write_line(&msg.to_string(), C_AGENT);
}

pub(crate) fn write_result(renderer: &mut Renderer, msg: impl std::fmt::Display) {
    let _ = renderer.write_line(&msg.to_string(), C_RESULT);
}

pub(crate) fn write_error(renderer: &mut Renderer, msg: impl std::fmt::Display) {
    let _ = renderer.write_line(&msg.to_string(), C_ERROR);
}

pub fn undo_last(session: &mut Session) -> usize {
    let len = session.messages.len();
    if len == 0 {
        return 0;
    }
    if session.messages[len - 1].role == MessageRole::Assistant {
        session.messages.pop();
        if session
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::User)
        {
            session.messages.pop();
            return 2;
        }
        return 1;
    }
    if session.messages[len - 1].role == MessageRole::User {
        session.messages.pop();
        return 1;
    }
    0
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_compress(
    instructions: Option<&str>,
    agent: &mut Option<AnyAgent>,
    client: &mut AnyClient,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &mut ContextFiles,
    reasoning_enabled: bool,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    #[cfg(feature = "mcp")] mcp_manager: Option<&crate::extras::mcp::McpClientManager>,
) -> anyhow::Result<()> {
    renderer.write_line("compressing...", C_AGENT)?;
    renderer.write_line("", crossterm::style::Color::White)?;

    let reserve = cfg.resolve_reserve_tokens();
    let keep_recent = cfg.resolve_keep_recent_tokens();
    let max_tokens = session.context_window.saturating_sub(reserve);

    if session.total_estimated_tokens <= max_tokens {
        renderer.write_line("context within limits, no compression needed", C_AGENT)?;
        return Ok(());
    }

    let mut accumulated = 0u64;
    let mut cut_idx = session.messages.len();
    for (i, msg) in session.messages.iter().enumerate().rev() {
        if accumulated >= keep_recent {
            cut_idx = i + 1;
            break;
        }
        accumulated = accumulated.saturating_add(msg.estimated_tokens);
    }

    if cut_idx == 0 {
        renderer.write_line("nothing to compress (entire context is recent)", C_AGENT)?;
        return Ok(());
    }

    let messages_to_summarize = &session.messages[..cut_idx];
    let previous_summary = session.compactions.last().map(|c| c.summary.as_str());

    let summary = client
        .compress_messages(
            &session.model,
            messages_to_summarize,
            previous_summary,
            instructions,
        )
        .await?;

    let tokens_before: u64 = messages_to_summarize
        .iter()
        .map(|m| m.estimated_tokens)
        .sum();

    #[cfg(feature = "memory")]
    crate::extras::memory::flush_compaction_summary(
        &crate::extras::memory::Mem::open(),
        &summary,
        Some(cut_idx), // = first_kept_index: how many messages were summarized
    );
    session.compress(summary, cut_idx, tokens_before);

    let model = client.completion_model(session.model.to_string());
    *agent = Some(
        crate::provider::build_agent(
            model,
            cli,
            cfg,
            context,
            permission.clone(),
            ask_tx.clone(),
            sandbox.clone(),
            reasoning_enabled,
            #[cfg(feature = "mcp")]
            mcp_manager,
        )
        .await,
    );
    renderer.write_line("prompt cleared (back to default behavior)", C_AGENT)?;

    render_session(renderer, session, cli, cfg, context)?;
    renderer.write_line(
        &format!(
            "compressed {} messages (saved ~{} tokens)",
            cut_idx, tokens_before,
        ),
        C_AGENT,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_slash(
    text: &str,
    agent: &mut Option<AnyAgent>,
    client: &mut AnyClient,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &mut ContextFiles,
    show_reasoning: &mut bool,
    reasoning_enabled: &mut bool,
    is_running: &mut bool,
    input: &mut InputEditor,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    todo_tools_enabled: &mut bool,
    sandbox: &Sandbox,
    #[cfg(feature = "loop")] loop_state: &mut Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "mcp")] mcp_manager: Option<&crate::extras::mcp::McpClientManager>,
) -> anyhow::Result<()> {
    let parts: SmallVec<[&str; 3]> = text.trim().splitn(3, ' ').collect();
    let mut ctx = SlashCtx {
        agent,
        client,
        renderer,
        session,
        cli,
        cfg,
        context,
        show_reasoning,
        reasoning_enabled,
        is_running,
        input,
        permission,
        ask_tx,
        todo_tools_enabled,
        sandbox,
        #[cfg(feature = "loop")]
        loop_state,
        #[cfg(feature = "mcp")]
        mcp_manager,
    };

    match parts[0] {
        "/provider" | "/model" | "/models" | "/models-add" | "/model-subagent"
        | "/models-subagent" => providers::handle(&parts, &mut ctx).await,
        "/prompt" | "/theme" | "/regen-prompts" | "/regen-themes" => {
            content::handle(&parts, &mut ctx).await
        }
        "/reasoning" | "/thinking" | "/mode" | "/toggle" | "/mcp" | "/editsys" => {
            settings::handle(&parts, &mut ctx).await
        }
        "/sessions" | "/clear" | "/new" | "/undo" | "/retry" | "/quit" | "/exit" | "/history" => {
            session::handle(&parts, &mut ctx).await
        }
        "/help" => {
            help::handle(&parts, &mut ctx);
            Ok(())
        }
        "/welcome" | "/tutorial" => {
            help::handle_welcome(ctx.renderer);
            Ok(())
        }
        "/add" | "/drop" | "/drop-all" => add::handle(&parts, &mut ctx).await,
        "/init" => init::handle(&parts, &mut ctx).await,
        "/memory" => memory::handle(&parts, &mut ctx).await,
        "/compress" | "/compact" | "/loop" | "/worktree" | "/wt-merge" | "/wt-exit" => {
            features::handle(&parts, &mut ctx).await
        }
        _ => {
            write_error(
                ctx.renderer,
                format!("unknown command: {} (try /help)", parts[0]),
            );
            Ok(())
        }
    }
}
