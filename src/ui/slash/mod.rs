pub(crate) mod add;
mod content;
mod features;
mod help;
#[cfg(feature = "hooks")]
mod hooks;
pub(crate) mod init;
mod memory;
mod providers;
pub(crate) mod review;
mod session;
pub(crate) mod settings;

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
use crate::ui::state::{AgentRunState, ChainState, SlashState, UiContext};

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
        let temperature =
            crate::config::resolve_temperature(self.cli, self.cfg, &self.session.model);
        let extra_body = crate::config::resolve_extra_body(self.cfg, &self.session.model);
        #[cfg(feature = "advisor")]
        {
            crate::extras::advisor::update_client(self.client.clone());
            crate::extras::advisor::set_session_messages(self.session.messages.clone());
        }
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
                temperature,
                extra_body,
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
        let temperature =
            crate::config::resolve_temperature(self.cli, self.cfg, &self.session.model);
        let extra_body = crate::config::resolve_extra_body(self.cfg, &self.session.model);
        #[cfg(feature = "advisor")]
        {
            crate::extras::advisor::update_client(self.client.clone());
            crate::extras::advisor::set_session_messages(self.session.messages.clone());
        }
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
                temperature,
                extra_body,
                #[cfg(feature = "mcp")]
                self.mcp_manager,
            )
            .await,
        );
        Ok(())
    }

    /// Switch to the quick-model configured in `[prompt_to_model]` for the
    /// given prompt name. Returns `true` if a model switch occurred (and the
    /// agent was rebuilt). When `false`, the caller should call
    /// `rebuild_agent()` to pick up other prompt changes (mode directive, etc.).
    pub async fn switch_to_prompt_model(&mut self, prompt_name: &str) -> bool {
        let qm_name = match self.cfg.resolve_prompt_model(prompt_name) {
            Some(name) => name,
            None => return false,
        };

        let qm = crate::config::quick_models_map(self.cfg);
        let Some(qmc) = qm.get(qm_name) else {
            return false;
        };

        let new_model = compact_str::CompactString::from(&*qmc.model);
        let provider_changed = qmc.provider != self.session.provider;

        // Update model before rebuild so the agent is built with it.
        self.session.model = new_model.clone();

        if provider_changed {
            match self
                .rebuild_agent_with_client(&qmc.provider, *self.reasoning_enabled)
                .await
            {
                Ok(()) => {
                    self.session.provider = compact_str::CompactString::from(&*qmc.provider);
                }
                Err(e) => {
                    let _ = self.renderer.write_line(
                        &format!(
                            "failed to switch provider for prompt '{}': {}",
                            prompt_name, e
                        ),
                        C_ERROR,
                    );
                    return false;
                }
            }
        } else {
            let model = self.client.completion_model(new_model.to_string());
            let temperature = crate::config::resolve_temperature(self.cli, self.cfg, &new_model);
            let extra_body = crate::config::resolve_extra_body(self.cfg, &new_model);
            #[cfg(feature = "advisor")]
            {
                crate::extras::advisor::update_client(self.client.clone());
                crate::extras::advisor::set_session_messages(self.session.messages.clone());
            }
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
                    temperature,
                    extra_body,
                    #[cfg(feature = "mcp")]
                    self.mcp_manager,
                )
                .await,
            );
        }

        self.session.input_token_cost = qmc.input_token_cost;
        self.session.output_token_cost = qmc.output_token_cost;
        self.session
            .update_context_window(self.cfg.resolve_context_window(
                &self.session.provider,
                &self.session.model,
                &crate::config::quick_models_map(self.cfg),
            ));

        let _ = self.renderer.write_line(
            &format!(
                "switched to model: {} (from prompt '{}')",
                qm_name, prompt_name
            ),
            C_AGENT,
        );
        true
    }
}

/// Free-function variant of [`SlashCtx::switch_to_prompt_model`] for call
/// sites that don't have a `SlashCtx` (dot commands, chain transitions,
/// startup). Returns `true` if a model switch occurred.
pub(crate) async fn apply_prompt_model(
    prompt_name: &str,
    ui: &mut UiContext<'_>,
    agent: &mut Option<AnyAgent>,
    reasoning_enabled: bool,
    renderer: &mut Renderer,
) -> bool {
    let qm_name = match ui.cfg.resolve_prompt_model(prompt_name) {
        Some(name) => name,
        None => return false,
    };

    let qm = crate::config::quick_models_map(ui.cfg);
    let Some(qmc) = qm.get(qm_name) else {
        return false;
    };

    let new_model = compact_str::CompactString::from(&*qmc.model);
    let provider_changed = qmc.provider != ui.session.provider;

    ui.session.model = new_model.clone();

    if provider_changed {
        match crate::provider::create_client(
            &qmc.provider,
            ui.cli.api_key.as_deref(),
            &ui.cfg.custom_providers_map(),
            ui.cfg.api_keys.as_ref(),
        ) {
            Ok(new_client) => {
                ui.client = new_client;
                ui.session.provider = compact_str::CompactString::from(&*qmc.provider);
                // Fall through to rebuild agent below
            }
            Err(e) => {
                let _ = renderer.write_line(
                    &format!(
                        "failed to switch provider for prompt '{}': {}",
                        prompt_name, e
                    ),
                    C_ERROR,
                );
                return false;
            }
        }
    }

    let model = ui.client.completion_model(new_model.to_string());
    let temperature = crate::config::resolve_temperature(ui.cli, ui.cfg, &new_model);
    let extra_body = crate::config::resolve_extra_body(ui.cfg, &new_model);
    #[cfg(feature = "advisor")]
    {
        crate::extras::advisor::update_client(ui.client.clone());
        crate::extras::advisor::set_session_messages(ui.session.messages.clone());
    }
    *agent = Some(
        crate::provider::build_agent(
            model,
            ui.cli,
            ui.cfg,
            ui.context,
            ui.permission.clone(),
            ui.ask_tx.clone(),
            ui.sandbox.clone(),
            reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            ui.mcp_manager.as_ref(),
        )
        .await,
    );

    ui.session.input_token_cost = qmc.input_token_cost;
    ui.session.output_token_cost = qmc.output_token_cost;
    ui.session
        .update_context_window(ui.cfg.resolve_context_window(
            &ui.session.provider,
            &ui.session.model,
            &qm,
        ));

    let _ = renderer.write_line(
        &format!(
            "switched to model: {} (from prompt '{}')",
            qm_name, prompt_name
        ),
        C_AGENT,
    );
    true
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
    let removed = if session.messages[len - 1].role == MessageRole::Assistant {
        if len >= 2 && session.messages[len - 2].role == MessageRole::User {
            2
        } else {
            1
        }
    } else if session.messages[len - 1].role == MessageRole::User {
        1
    } else {
        0
    };
    // Rewind via the session helper so the context figure tracks the shortened
    // history (subtracts the removed turn from the calibration anchor rather than
    // going stale or resetting to a cold estimate) and the cut is reversible with
    // /redo.
    if removed > 0 {
        session.rewind_to(len - removed);
    }
    removed
}

pub async fn handle_compress(
    instructions: Option<&str>,
    auto: bool,
    agent: &mut Option<AnyAgent>,
    renderer: &mut Renderer,
    ui: &mut UiContext<'_>,
    reasoning_enabled: bool,
) -> anyhow::Result<()> {
    // Mirror the auto-compaction trigger's reserve exactly (including memory's
    // effective_reserve) so the budget gate here can never disagree with the
    // gate that decided to call us.
    let qm = crate::config::quick_models_map(ui.cfg);
    #[cfg(feature = "memory")]
    let reserve = crate::extras::memory::effective_reserve(
        ui.cfg.resolve_reserve_tokens(&ui.session.model, &qm),
        ui.context.memory.as_deref(),
    );
    #[cfg(not(feature = "memory"))]
    let reserve = ui.cfg.resolve_reserve_tokens(&ui.session.model, &qm);
    let keep_recent = ui.cfg.resolve_keep_recent_tokens();
    let max_tokens = ui.session.context_window.saturating_sub(reserve);

    // Auto-compaction only makes sense when actually over budget; manual
    // /compress is the user's explicit intent, so it skips the budget gate and
    // proceeds regardless of how full the context is.
    if auto && ui.session.effective_context_tokens() <= max_tokens {
        return Ok(());
    }

    let cut_idx = crate::session::Session::select_compaction_cut(&ui.session.messages, keep_recent);

    // Nothing old enough to summarize (everything is within keep_recent). This
    // is a real physical limit even when forced, so report it for manual runs;
    // stay silent under auto so an over-budget-but-unsummarizable turn does not
    // announce a no-op on every completion.
    if cut_idx == 0 {
        if !auto {
            renderer.write_line("not enough conversation history to compact yet", C_AGENT)?;
        }
        return Ok(());
    }

    // Announce only once we know compression will actually run.
    if auto {
        renderer.write_line("auto-compacting...", crossterm::style::Color::DarkGrey)?;
    } else {
        renderer.write_line("compressing...", C_AGENT)?;
    }
    renderer.write_line("", crossterm::style::Color::White)?;

    let messages_to_summarize = &ui.session.messages[..cut_idx];
    let previous_summary = ui.session.compactions.last().map(|c| c.summary.as_str());

    let summary = ui
        .client
        .compress_messages(
            &ui.session.model,
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
    ui.session.compress(summary, cut_idx, tokens_before);

    let model = ui.client.completion_model(ui.session.model.to_string());
    let temperature = crate::config::resolve_temperature(ui.cli, ui.cfg, &ui.session.model);
    let extra_body = crate::config::resolve_extra_body(ui.cfg, &ui.session.model);
    *agent = Some(
        crate::provider::build_agent(
            model,
            ui.cli,
            ui.cfg,
            ui.context,
            ui.permission.clone(),
            ui.ask_tx.clone(),
            ui.sandbox.clone(),
            reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            ui.mcp_manager.as_ref(),
        )
        .await,
    );

    render_session(renderer, ui.session, ui.cli, ui.cfg, ui.context)?;
    renderer.write_line(
        &format!(
            "compressed {} messages (saved ~{} tokens)",
            cut_idx, tokens_before,
        ),
        C_AGENT,
    )?;

    Ok(())
}

pub async fn handle_slash(
    text: &str,
    renderer: &mut Renderer,
    input: &mut InputEditor,
    run: &mut AgentRunState,
    ui: &mut UiContext<'_>,
    slash: &mut SlashState,
    chain: &mut ChainState,
) -> anyhow::Result<()> {
    // `chain` only feeds `SlashCtx::loop_state`; without the loop feature it
    // has no consumer here.
    #[cfg(not(feature = "loop"))]
    let _ = &chain;
    let parts: SmallVec<[&str; 3]> = text.trim().splitn(3, ' ').collect();
    let mut ctx = SlashCtx {
        agent: &mut run.agent,
        client: &mut ui.client,
        renderer,
        session: ui.session,
        cli: ui.cli,
        cfg: ui.cfg,
        context: ui.context,
        show_reasoning: &mut slash.show_reasoning,
        reasoning_enabled: &mut slash.reasoning_enabled,
        is_running: &mut run.is_running,
        input,
        permission: &ui.permission,
        ask_tx: &ui.ask_tx,
        todo_tools_enabled: &mut slash.todo_tools_enabled,
        sandbox: &ui.sandbox,
        #[cfg(feature = "loop")]
        loop_state: &mut chain.loop_state,
        #[cfg(feature = "mcp")]
        mcp_manager: ui.mcp_manager.as_ref(),
    };

    match parts[0] {
        "/provider" | "/model" | "/models" | "/models-add" | "/model-subagent"
        | "/models-subagent" => providers::handle(&parts, &mut ctx).await,
        "/prompt" | "/theme" | "/regen-prompts" | "/regen-themes" => {
            content::handle(&parts, &mut ctx).await
        }
        "/reasoning" | "/thinking" | "/mode" | "/toggle" | "/mcp" | "/editsys" | "/advisor" => {
            settings::handle(&parts, &mut ctx).await
        }
        "/sessions" | "/rename" | "/clear" | "/new" | "/undo" | "/redo" | "/rewind" | "/retry"
        | "/quit" | "/exit" | "/history" => session::handle(&parts, &mut ctx).await,
        "/help" => {
            help::handle(&parts, &mut ctx);
            Ok(())
        }
        "/welcome" | "/tutorial" => {
            help::handle_welcome(ctx.renderer);
            Ok(())
        }
        "/tutor" => {
            help::handle_tutor(ctx.renderer);
            Ok(())
        }
        "/add" | "/drop" | "/drop-all" => add::handle(&parts, &mut ctx).await,
        "/init" => init::handle(&parts, &mut ctx).await,
        "/review" => review::handle(&parts, &mut ctx).await,
        "/memory" => memory::handle(&parts, &mut ctx).await,
        "/compress" | "/compact" | "/loop" | "/worktree" | "/wt-merge" | "/wt-exit" => {
            features::handle(&parts, &mut ctx).await
        }
        #[cfg(feature = "hooks")]
        "/hooks" => hooks::handle(&parts, &mut ctx).await,
        _ => {
            write_error(
                ctx.renderer,
                format!("unknown command: {} (try /help)", parts[0]),
            );
            Ok(())
        }
    }
}
