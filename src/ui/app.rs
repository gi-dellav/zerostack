use std::collections::VecDeque;
use std::io::{self, Write};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::config::{self, Config};
use crate::context::ContextFiles;
use crate::event::{AgentEvent, BtwEvent, UserEvent};
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::extras::status_signals::StatusSignals;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::event_handler;
use crate::ui::events::{render_session, sanitize_output};
use crate::ui::input::InputEditor;
use crate::ui::permission_handler::handle_permission_request;
use crate::ui::pickers::rewind::RewindOutcome;
use crate::ui::renderer::{self as renderer_mod, ChainPrompt, Renderer, copy_to_clipboard};
use crate::ui::slash::{apply_prompt_model, handle_compress, handle_slash};
use crate::ui::terminal::TerminalGuard;
use crate::ui::utils::{parse_color, to_ansi_256};

#[cfg(feature = "mcp")]
use super::ensure_mcp_manager;
#[cfg(feature = "advisor")]
use super::handle_human_handoff;
#[cfg(feature = "git-worktree")]
use super::spawn_merge_agent;
use super::{
    C_AGENT, C_BTW, C_ERROR, C_TOOL, PrebuildPayload, classify_submission,
    mid_turn_compact_and_respawn, refresh_display, spawn_event_thread, start_main_run,
    stop_turn_context_exhausted,
};
#[cfg(feature = "git-worktree")]
use super::{C_PERM, apply_current_prompt_mode};

const TURN_TRACE_MAX: usize = 64;

pub(crate) struct App<'a> {
    cli: &'a Cli,
    cfg: &'a Config,
    session: &'a mut Session,
    context: &'a mut ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    ask_rx: Option<mpsc::Receiver<crate::permission::ask::AskRequest>>,
    sandbox: Sandbox,
    status_signals: Option<StatusSignals>,
    #[cfg(feature = "advisor")]
    handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,

    client: AnyClient,
    agent: Option<AnyAgent>,
    renderer: Renderer,
    input: InputEditor,
    last_branch_check: std::time::Instant,
    #[cfg(feature = "mcp")]
    mcp_manager: Option<McpClientManager>,

    is_running: bool,
    agent_rx: Option<mpsc::Receiver<AgentEvent>>,
    main_abort: Option<tokio::task::AbortHandle>,
    pending_inputs: VecDeque<String>,
    agent_line_started: bool,
    response_buf: String,
    response_start_block: Option<usize>,
    show_reasoning: bool,
    reasoning_enabled: bool,
    pending_send: Option<String>,
    was_reasoning: bool,
    todo_tools_enabled: bool,
    loop_label: Option<String>,
    #[cfg(feature = "loop")]
    loop_state: Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "git-worktree")]
    wt_return_path: Option<(String, String, String, bool)>,

    btw_tx: mpsc::Sender<BtwEvent>,
    btw_rx: mpsc::Receiver<BtwEvent>,
    btw_abort: Vec<(u32, tokio::task::AbortHandle)>,
    btw_inflight: usize,
    btw_next_id: u32,
    btw_total_cost: f64,
    btw_total_in: u64,
    btw_total_out: u64,

    turn_trace: Vec<compact_str::CompactString>,
    awaiting_compaction_relief: bool,
    dot_prompt_restore: Option<String>,
    chain_pending: Option<crate::extras::chain::ChainPhase>,
    chain_label_msg: Option<String>,

    user_tx: mpsc::Sender<UserEvent>,
    user_rx: mpsc::Receiver<UserEvent>,
    running: Arc<AtomicBool>,
    event_handle: Option<std::thread::JoinHandle<()>>,
    prebuild_rx: Option<mpsc::Receiver<PrebuildPayload>>,
    _terminal_guard: TerminalGuard,
}

impl<'a> App<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        client: AnyClient,
        mut agent: Option<AnyAgent>,
        cli: &'a Cli,
        cfg: &'a Config,
        session: &'a mut Session,
        context: &'a mut ContextFiles,
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        ask_rx: Option<mpsc::Receiver<crate::permission::ask::AskRequest>>,
        sandbox: Sandbox,
        auto_trigger_msg: Option<String>,
        status_signals: Option<StatusSignals>,
        #[cfg(feature = "advisor")] handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,
    ) -> anyhow::Result<Self> {
        let _terminal_guard = TerminalGuard::new()?;

        session.show_cost_always = cfg.resolve_show_cost_always();
        crate::ui::statusline::init(cfg);

        session.refresh_git_branch();
        if crate::ui::statusline::needs_git_status() {
            session.refresh_git_status();
        }
        let last_branch_check = std::time::Instant::now();

        #[cfg(feature = "mcp")]
        let mut mcp_manager: Option<McpClientManager> = None;

        let mut renderer = Renderer::new()?;
        renderer.set_statusline_height(crate::ui::statusline::line_count());
        renderer.set_monochrome(cli.no_color);
        renderer.set_chat_margin(cfg.resolve_chat_left_margin());
        if let Some(ref theme_name) = context.current_theme_name {
            if let Some(content) = context.themes.get(theme_name.as_str()) {
                crate::context::themes::apply(content, &mut renderer);
            }
        } else if let Some(colors) = &cfg.colors {
            let chat_bg = colors.chat_background.as_deref().and_then(parse_color);
            let input_bg = colors.input_background.as_deref().and_then(parse_color);
            let status_bg = colors.status_background.as_deref().and_then(parse_color);
            if matches!(colors.scheme_type, config::SchemeType::Ansi) {
                renderer.set_background_colors(
                    chat_bg.map(to_ansi_256),
                    input_bg.map(to_ansi_256),
                    status_bg.map(to_ansi_256),
                );
            } else {
                renderer.set_background_colors(chat_bg, input_bg, status_bg);
            }
        }

        let mut input = InputEditor::new();
        input.set_monochrome(cli.no_color);
        let mut prompt_names: Vec<String> = context.prompts.keys().cloned().collect();
        prompt_names.sort();
        input.set_prompt_names(prompt_names);
        input.set_prompt_sources(
            context
                .prompt_sources
                .iter()
                .map(|(name, src)| (name.clone(), src.label().to_string()))
                .collect(),
        );
        input.set_theme_names(context.themes.keys().cloned().collect());
        if let Some(editor) = &cfg.editor {
            input.set_editor(editor.clone());
        }
        input.set_quick_model_names(config::quick_models_map(cfg).into_keys().collect());
        {
            let mut providers: Vec<String> =
                ["anthropic", "openai", "gemini", "openrouter", "ollama"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            providers.extend(cfg.custom_providers_map().keys().cloned());
            input.set_provider_names(providers);
        }
        input.load_global_history();

        let mut is_running = false;
        let mut agent_rx: Option<mpsc::Receiver<AgentEvent>> = None;
        let mut main_abort: Option<tokio::task::AbortHandle> = None;
        let pending_inputs: VecDeque<String> = VecDeque::new();
        let agent_line_started = false;
        let response_buf = String::new();
        let response_start_block: Option<usize> = None;
        let show_reasoning = cfg.resolve_show_reasoning();
        let reasoning_enabled = true;
        session.reasoning_enabled = reasoning_enabled;
        let pending_send: Option<String> = None;
        let was_reasoning = false;
        let todo_tools_enabled = false;
        let loop_label: Option<String> = None;
        #[cfg(feature = "loop")]
        let loop_state: Option<crate::extras::r#loop::LoopState> = None;
        #[cfg(feature = "git-worktree")]
        let wt_return_path: Option<(String, String, String, bool)> = None;

        session.overhead_tokens =
            crate::agent::builder::estimate_overhead(context, reasoning_enabled);

        let perm_mode = || -> Option<String> {
            permission.as_ref().map(|p| {
                p.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .mode()
                    .to_string()
            })
        };

        render_session(&mut renderer, session, cli, cfg, context)?;
        let marker_path = crate::session::storage::data_dir().join("shown_welcome_msg");
        if cfg.resolve_always_show_welcome() || !marker_path.exists() {
            crate::ui::events::show_welcome(&mut renderer)?;
            if !cfg.resolve_always_show_welcome() {
                if let Some(dir) = marker_path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&marker_path, "");
            }
        }
        refresh_display(
            &mut renderer,
            &mut input,
            session,
            false,
            None,
            context.current_prompt_name.as_deref(),
            perm_mode().as_deref(),
            None,
            0.0,
            0,
            0,
        )?;

        {
            let provider = session.provider.to_string();
            let is_custom = cfg.custom_providers_map().contains_key(&provider);
            let ids =
                crate::ui::slash::warm_model_cache(&provider, is_custom, &client, cli, cfg).await;
            input.set_live_model_names(ids);
        }

        #[cfg(feature = "git-worktree")]
        if let Some(name) = &cli.worktree {
            let wt_base_dir = cli.resolve_wt_base_dir(cfg);
            match crate::extras::git_worktree::create(name, wt_base_dir.as_deref()) {
                Ok((path, _info)) => {
                    std::env::set_current_dir(&path).ok();
                    session.working_dir = compact_str::CompactString::new(path.to_string_lossy());
                    context.reload();
                    apply_current_prompt_mode(context, &permission);
                    #[cfg(feature = "mcp")]
                    let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                    let model = client.completion_model(session.model.to_string());
                    let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
                    let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
                    agent = Some(
                        crate::provider::build_agent(
                            model,
                            cli,
                            cfg,
                            context,
                            permission.clone(),
                            ask_tx.clone(),
                            sandbox.clone(),
                            reasoning_enabled,
                            temperature,
                            extra_body,
                            #[cfg(feature = "mcp")]
                            mcp_ref,
                        )
                        .await,
                    );
                    let _ = render_session(&mut renderer, session, cli, cfg, context);
                }
                Err(e) => {
                    let _ = renderer.write_line(&format!("worktree failed: {}", e), C_ERROR);
                }
            }
        }
        #[cfg(feature = "git-worktree")]
        if cli.parallel {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let name = ts.to_string();
            let wt_base_dir = cli.resolve_wt_base_dir(cfg);
            match crate::extras::git_worktree::create(&name, wt_base_dir.as_deref()) {
                Ok((path, _info)) => {
                    std::env::set_current_dir(&path).ok();
                    session.working_dir = compact_str::CompactString::new(path.to_string_lossy());
                    context.reload();
                    apply_current_prompt_mode(context, &permission);
                    #[cfg(feature = "mcp")]
                    let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                    let model = client.completion_model(session.model.to_string());
                    let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
                    let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
                    agent = Some(
                        crate::provider::build_agent(
                            model,
                            cli,
                            cfg,
                            context,
                            permission.clone(),
                            ask_tx.clone(),
                            sandbox.clone(),
                            reasoning_enabled,
                            temperature,
                            extra_body,
                            #[cfg(feature = "mcp")]
                            mcp_ref,
                        )
                        .await,
                    );
                    let _ = render_session(&mut renderer, session, cli, cfg, context);
                }
                Err(e) => {
                    let _ = renderer.write_line(&format!("worktree failed: {}", e), C_ERROR);
                }
            }
        }

        if let Some(ref trigger_msg) = auto_trigger_msg {
            for line in trigger_msg.lines() {
                let safe_line = sanitize_output(line);
                renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
            }
            renderer.write_line("", Color::White)?;

            #[cfg(feature = "mcp")]
            let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
            event_handler::ensure_agent(
                &mut agent,
                &client,
                session,
                cli,
                cfg,
                context,
                &permission,
                &ask_tx,
                &sandbox,
                reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_ref,
            )
            .await;
            let history = crate::agent::runner::convert_history(session);
            let runner = agent
                .as_ref()
                .unwrap()
                .clone()
                .spawn_runner(
                    trigger_msg.to_string(),
                    history,
                    cfg.retry.clone(),
                    #[cfg(feature = "hooks")]
                    None,
                )
                .await;
            agent_rx = Some(runner.event_rx);
            main_abort = Some(runner.abort_handle);
            is_running = true;
            if let Some(ss) = status_signals.as_ref() {
                ss.send_start();
            }
            session.add_message(MessageRole::User, trigger_msg);
            #[cfg(feature = "advisor")]
            crate::extras::advisor::set_session_messages(session.messages.clone());
        }

        let (user_tx, user_rx) = mpsc::channel::<UserEvent>(64);
        let running = Arc::new(AtomicBool::new(true));
        let event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));

        let (prebuild_tx, prebuild_rx_raw) = mpsc::channel::<PrebuildPayload>(1);
        let prebuild_rx = Some(prebuild_rx_raw);
        if auto_trigger_msg.is_none() && agent.is_none() {
            let client_clone = client.clone();
            let session_model = session.model.to_string();
            let cli_clone = cli.clone();
            let cfg_clone = cfg.clone();
            let context_clone = context.clone();
            let permission_clone = permission.clone();
            let ask_tx_clone = ask_tx.clone();
            let sandbox_clone = sandbox.clone();
            tokio::spawn(async move {
                #[cfg(feature = "mcp")]
                let mcp = if let Some(ref servers) = cfg_clone.mcp_servers {
                    if !servers.is_empty() {
                        Some(McpClientManager::connect_all(servers).await)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let model = client_clone.completion_model(session_model.clone());
                let temperature =
                    crate::config::resolve_temperature(&cli_clone, &cfg_clone, &session_model);
                let extra_body = crate::config::resolve_extra_body(&cfg_clone, &session_model);
                let a = crate::provider::build_agent(
                    model,
                    &cli_clone,
                    &cfg_clone,
                    &context_clone,
                    permission_clone,
                    ask_tx_clone,
                    sandbox_clone,
                    reasoning_enabled,
                    temperature,
                    extra_body,
                    #[cfg(feature = "mcp")]
                    mcp.as_ref(),
                )
                .await;

                #[cfg(feature = "mcp")]
                let _ = prebuild_tx.send((a, mcp)).await;
                #[cfg(not(feature = "mcp"))]
                let _ = prebuild_tx.send(a).await;
            });
        }

        let (btw_tx, btw_rx) = mpsc::channel::<BtwEvent>(32);

        Ok(Self {
            cli,
            cfg,
            session,
            context,
            permission,
            ask_tx,
            ask_rx,
            sandbox,
            status_signals,
            #[cfg(feature = "advisor")]
            handoff_rx,
            client,
            agent,
            renderer,
            input,
            last_branch_check,
            #[cfg(feature = "mcp")]
            mcp_manager,
            is_running,
            agent_rx,
            main_abort,
            pending_inputs,
            agent_line_started,
            response_buf,
            response_start_block,
            show_reasoning,
            reasoning_enabled,
            pending_send,
            was_reasoning,
            todo_tools_enabled,
            loop_label,
            #[cfg(feature = "loop")]
            loop_state,
            #[cfg(feature = "git-worktree")]
            wt_return_path,
            btw_tx,
            btw_rx,
            btw_abort: Vec::new(),
            btw_inflight: 0,
            btw_next_id: 0,
            btw_total_cost: 0.0,
            btw_total_in: 0,
            btw_total_out: 0,
            turn_trace: Vec::new(),
            awaiting_compaction_relief: false,
            dot_prompt_restore: None,
            chain_pending: None,
            chain_label_msg: None,
            user_tx,
            user_rx,
            running,
            event_handle,
            prebuild_rx,
            _terminal_guard,
        })
    }

    pub(crate) async fn run(&mut self) -> anyhow::Result<()> {
        loop {
            self.session.reasoning_enabled = self.reasoning_enabled;
            if self.last_branch_check.elapsed() >= Duration::from_secs(1) {
                self.session.refresh_git_branch();
                if crate::ui::statusline::needs_git_status() {
                    self.session.refresh_git_status();
                }
                self.last_branch_check = std::time::Instant::now();
            }

            tokio::select! {
                Some(ev) = self.user_rx.recv() => {
                    match self.handle_user_event(ev).await? {
                        ControlFlow::Break(()) => break,
                        ControlFlow::Continue(()) => {}
                    }
                }
                Some(prebuilt) = async { self.prebuild_rx.as_mut()?.recv().await }, if self.agent.is_none() => {
                    self.take_prebuild(prebuilt, true)?;
                    self.refresh()?;
                }
                Some(event) = async { self.agent_rx.as_mut()?.recv().await } => {
                    self.handle_agent_event(event).await?;
                }
                Some(ask_req) = async { self.ask_rx.as_mut()?.recv().await } => {
                    handle_permission_request(
                        ask_req,
                        &mut self.renderer,
                        self.session,
                        self.cli,
                        &mut self.user_rx,
                        &mut self.agent_line_started,
                        &mut self.was_reasoning,
                    ).await?;
                    self.refresh()?;
                }
                Some(bev) = self.btw_rx.recv() => {
                    self.handle_btw_event(bev)?;
                    self.refresh()?;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)), if self.is_running => {
                    self.refresh()?;
                }
                else => {
                    if let Some(rx) = self.prebuild_rx.as_mut()
                        && self.agent.is_none()
                        && let Ok(payload) = rx.try_recv()
                    {
                        self.take_prebuild(payload, false)?;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }

            #[cfg(feature = "advisor")]
            if let Some(ref mut rx) = self.handoff_rx
                && let Ok(req) = rx.try_recv()
            {
                handle_human_handoff(
                    req,
                    &mut self.renderer,
                    &mut self.user_rx,
                    &mut self.agent_line_started,
                    &mut self.was_reasoning,
                )
                .await?;
                self.refresh()?;
            }
        }

        self.handle_worktree_auto_merge().await?;
        Ok(())
    }

    pub(crate) async fn teardown(self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.event_handle {
            let _ = h.join();
        }
        #[cfg(feature = "mcp")]
        if let Some(mgr) = self.mcp_manager {
            mgr.shutdown().await;
        }
    }

    fn refresh(&mut self) -> io::Result<()> {
        let perm_mode = self.perm_mode();
        refresh_display(
            &mut self.renderer,
            &mut self.input,
            self.session,
            self.is_running,
            self.loop_label.as_deref(),
            self.context.current_prompt_name.as_deref(),
            perm_mode.as_deref(),
            self.chain_label_msg.as_deref(),
            self.btw_total_cost,
            self.btw_total_in,
            self.btw_total_out,
        )
    }

    fn perm_mode(&self) -> Option<String> {
        self.permission.as_ref().map(|p| {
            p.lock()
                .unwrap_or_else(|e| e.into_inner())
                .mode()
                .to_string()
        })
    }

    async fn handle_user_event(&mut self, ev: UserEvent) -> anyhow::Result<ControlFlow<(), ()>> {
        match ev {
            UserEvent::Resize => {
                self.renderer.resize();
            }
            UserEvent::ScrollUp => {
                if !self.renderer.input_scroll_up() {
                    self.renderer.scroll_line_up();
                }
            }
            UserEvent::ScrollDown => {
                if self.renderer.is_scrolling() {
                    self.renderer.scroll_line_down();
                } else {
                    self.renderer.input_scroll_down();
                }
            }
            UserEvent::MouseDown { row, col } => {
                if let Some(pos) =
                    self.renderer
                        .input_cursor_for_click(row, col, &self.input.buffer)
                {
                    self.input.set_cursor(pos);
                } else if row < self.renderer.visible_lines() as u16
                    && let Some(idx) = self.renderer.buffer_line_at_row(row)
                {
                    if let Some(url) = self.renderer.link_url_at(idx, col) {
                        renderer_mod::open_url(&url);
                    } else {
                        self.renderer.selection_active = true;
                        self.renderer.selection_start = Some(idx);
                        self.renderer.selection_end = Some(idx);
                    }
                }
            }
            UserEvent::MouseDrag { row, col: _ } => {
                if self.renderer.selection_active
                    && let Some(idx) = self.renderer.buffer_line_at_row(row)
                {
                    self.renderer.selection_end = Some(idx);
                }
            }
            UserEvent::MouseUp { row, col: _ } => {
                if self.renderer.selection_active {
                    if let Some(idx) = self.renderer.buffer_line_at_row(row) {
                        self.renderer.selection_end = Some(idx);
                    }
                    if let Some(text) = self.renderer.selected_text() {
                        copy_to_clipboard(&text);
                    }
                    self.renderer.clear_selection();
                }
            }
            UserEvent::Paste(data) => {
                self.input.handle_paste(data);
            }
            #[cfg(feature = "mcp")]
            UserEvent::McpLoginDone { server, error } => {
                self.handle_mcp_login_done(server, error).await?;
            }
            UserEvent::Key(key) => {
                let is_ctrl_c =
                    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                let is_ctrl_d =
                    key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL);
                if is_ctrl_c || is_ctrl_d {
                    if self.btw_inflight > 0 {
                        for (_, h) in self.btw_abort.drain(..) {
                            h.abort();
                        }
                        self.btw_inflight = 0;
                        self.renderer.write_line("btw cancelled", C_ERROR)?;
                    } else if self.is_running {
                        self.abort_main_run()?;
                    } else {
                        return Ok(ControlFlow::Break(()));
                    }
                    self.refresh()?;
                    return Ok(ControlFlow::Continue(()));
                }

                if let Err(e) = self.handle_key_event(key).await {
                    if e.downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::Interrupted)
                    {
                        return Ok(ControlFlow::Break(()));
                    }
                    return Err(e);
                }
            }
        }

        self.refresh()?;
        Ok(ControlFlow::Continue(()))
    }

    async fn handle_key_event(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        if self.renderer.selection_active && key.code == KeyCode::Char('y') {
            if let Some(text) = self.renderer.selected_text() {
                copy_to_clipboard(&text);
                self.renderer.write_line("copied selection", Color::Green)?;
            }
            self.renderer.clear_selection();
            return Ok(());
        }
        if self.renderer.selection_active && key.code == KeyCode::Esc {
            self.renderer.clear_selection();
            return Ok(());
        }

        let ctrl_r =
            key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_r {
            self.show_reasoning = !self.show_reasoning;
            self.renderer.write_line(
                &format!(
                    "reasoning visibility: {}",
                    if self.show_reasoning { "on" } else { "off" }
                ),
                Color::White,
            )?;
            return Ok(());
        }

        match key.code {
            KeyCode::PageUp => {
                self.renderer.scroll_page_up();
                return Ok(());
            }
            KeyCode::PageDown => {
                self.renderer.scroll_page_down();
                return Ok(());
            }
            KeyCode::Home => {
                self.renderer.scroll_to_top();
                return Ok(());
            }
            KeyCode::End => {
                self.renderer.scroll_to_bottom()?;
                return Ok(());
            }
            _ => {}
        }

        if self.input.picker.as_ref().is_some_and(|p| p.active())
            && self.input.handle_picker_key(key)
        {
            if let Some(RewindOutcome::Confirmed(idx)) = self.input.take_rewind_outcome() {
                let text = self
                    .session
                    .messages
                    .get(idx)
                    .map(|m| m.content.to_string());
                if self.session.rewind_to(idx) > 0 {
                    if let Some(text) = text {
                        self.input.load_text(&text);
                    }
                    if !self.cli.no_session {
                        let _ = crate::session::storage::save_session(self.session);
                    }
                    render_session(
                        &mut self.renderer,
                        self.session,
                        self.cli,
                        self.cfg,
                        self.context,
                    )?;
                    self.renderer
                        .write_line("rewound; /redo to restore", Color::Green)?;
                }
            }
            return Ok(());
        }

        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.rebind_event_thread();
            self.input.open_in_editor();
            return Ok(());
        }

        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.run_lazygit()?;
            return Ok(());
        }

        // Chain prompt active: intercept Y/N/B keystrokes
        if self.renderer.chain_prompt.is_some() && !self.renderer.chain_but_mode {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.renderer.chain_prompt = None;
                    if let Some(phase) = self.chain_pending.take() {
                        self.chain_label_msg = None;
                        self.run_chain_transition(phase, None).await?;
                    }
                    return Ok(());
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.renderer.chain_prompt = None;
                    self.chain_pending = None;
                    self.chain_label_msg = None;
                    self.renderer
                        .write_line("chain declined — won't ask again this session", C_AGENT)?;
                    if let Some(ref name) = self.context.current_prompt_name
                        && !self.context.chain_declined.contains(name)
                    {
                        self.context.chain_declined.push(name.clone());
                    }
                    return Ok(());
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    self.renderer.chain_but_mode = true;
                    self.renderer.chain_prompt = None;
                    self.input.clear_buffer();
                    self.chain_label_msg = self.chain_pending.map(|p| p.chain_label().to_string());
                    return Ok(());
                }
                _ => {
                    return Ok(());
                }
            }
        }

        // Chain but mode: Esc cancels back to ask
        if self.renderer.chain_but_mode && key.code == KeyCode::Esc {
            self.renderer.chain_but_mode = false;
            if let Some(phase) = self.chain_pending {
                self.renderer.chain_prompt = Some(ChainPrompt {
                    question: compact_str::CompactString::from(phase.chain_label()),
                });
                self.chain_label_msg = Some(phase.chain_label().to_string());
            }
            self.input.clear_buffer();
            return Ok(());
        }

        if let Some(mut text) = self.input.handle_key(key) {
            #[cfg(feature = "loop")]
            if self.loop_state.as_ref().is_some_and(|ls| ls.active) && !text.starts_with('/') {
                self.renderer
                    .write_line("loop active: /loop stop to cancel", C_ERROR)?;
                return Ok(());
            }
            if self.renderer.is_scrolling() {
                self.renderer.scroll_to_bottom()?;
            }

            // Chain-of-prompts: handle text submission after B (but) mode
            if !self.is_running
                && let Some(phase) = self.chain_pending.take()
            {
                self.chain_label_msg = None;
                self.renderer.chain_but_mode = false;
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    self.chain_pending = Some(phase);
                    self.chain_label_msg = Some(phase.chain_label().to_string());
                    self.renderer.chain_prompt = Some(ChainPrompt {
                        question: compact_str::CompactString::from(phase.chain_label()),
                    });
                    return Ok(());
                }
                self.run_chain_transition(phase, Some(&trimmed)).await?;
                return Ok(());
            }

            match classify_submission(self.is_running, &text) {
                super::SubmitAction::Run => {}
                super::SubmitAction::Ignore => {
                    return Ok(());
                }
                super::SubmitAction::RejectWhileRunning => {
                    self.renderer.write_line(
                        "agent is running — wait for it to finish or press Ctrl-C before running a command",
                        C_ERROR,
                    )?;
                    return Ok(());
                }
                super::SubmitAction::Queue => {
                    self.pending_inputs.push_back(text.to_string());
                    self.renderer
                        .write_line(&format!("queued: {}", sanitize_output(&text)), C_TOOL)?;
                    return Ok(());
                }
            }

            // Bypass-slot handlers: /queue and /btw
            {
                let t = text.trim_start();
                if t == "/queue" || t.starts_with("/queue ") {
                    let arg = t.strip_prefix("/queue").unwrap_or("").trim();
                    self.run_queue_command(arg)?;
                    return Ok(());
                }
            }
            {
                let t = text.trim_start();
                if t == "/btw" || t.starts_with("/btw ") {
                    self.run_btw(&text).await?;
                    return Ok(());
                }
            }

            if self.handle_dot_command(&mut text).await? {
                return Ok(());
            }

            if text.starts_with('/') {
                self.run_slash_command(&text).await?;
            } else if text.starts_with('!') {
                self.run_bang_command(&text).await?;
            } else {
                for line in text.lines() {
                    let safe_line = sanitize_output(line);
                    self.renderer
                        .write_line(&format!("> {}", safe_line), Color::Green)?;
                }
                self.renderer.write_line("", Color::White)?;
                self.start_main_run(&text).await;
            }
        }

        Ok(())
    }

    async fn handle_agent_event(&mut self, event: AgentEvent) -> anyhow::Result<()> {
        match &event {
            AgentEvent::ToolCall { name, args } => {
                if self.turn_trace.len() < TURN_TRACE_MAX {
                    self.turn_trace
                        .push(compact_str::CompactString::from(format!(
                            "→ {}",
                            crate::ui::utils::format_tool_call_summary(name, args)
                        )));
                }
            }
            AgentEvent::ToolResult { output, .. } => {
                if self.turn_trace.len() < TURN_TRACE_MAX {
                    self.turn_trace
                        .push(compact_str::CompactString::from(format!(
                            "← {}",
                            crate::extras::truncate::truncate_cjk(output, 500, "…")
                        )));
                }
            }
            AgentEvent::Done { .. } | AgentEvent::Error(_) => {
                self.turn_trace.clear();
                self.awaiting_compaction_relief = false;
            }
            _ => {}
        }

        #[cfg(feature = "loop")]
        let loop_running = self.loop_state.as_ref().is_some_and(|ls| ls.active);
        #[cfg(not(feature = "loop"))]
        let loop_running = false;

        if let AgentEvent::CompletionCall {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            ..
        } = &event
            && self.is_running
            && !loop_running
            && !self.cli.no_session
            && self.cfg.resolve_compact_enabled()
            && self.session.context_window > 0
            && let Some(threshold) = self.cfg.resolve_mid_turn_compact_threshold()
        {
            let real_input_tokens = crate::session::Session::real_input_tokens(
                self.cfg.is_anthropic_native(&self.session.provider),
                *input_tokens,
                *cached_input_tokens,
                *cache_creation_input_tokens,
            );
            let pressure = real_input_tokens as f64 / self.session.context_window as f64;
            if pressure > threshold {
                if self.awaiting_compaction_relief {
                    self.stop_context_exhausted(real_input_tokens, threshold)?;
                    self.awaiting_compaction_relief = false;
                } else {
                    self.mid_turn_compact(pressure).await?;
                    self.awaiting_compaction_relief = true;
                }
                self.refresh()?;
                return Ok(());
            } else {
                self.awaiting_compaction_relief = false;
            }
        }

        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        let turn_errored = matches!(&event, AgentEvent::Error(_));
        event_handler::handle_agent_event(
            event,
            &mut self.renderer,
            self.session,
            self.cfg,
            self.cli,
            self.context,
            &mut self.is_running,
            &mut self.agent_rx,
            &mut self.agent_line_started,
            &mut self.response_buf,
            &mut self.response_start_block,
            &mut self.was_reasoning,
            self.show_reasoning,
            &mut self.agent,
            &mut self.client,
            &mut self.loop_label,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            &self.status_signals,
            #[cfg(feature = "loop")]
            &mut self.loop_state,
            #[cfg(feature = "git-worktree")]
            &mut self.wt_return_path,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await?;

        self.finalize_turn(turn_errored).await?;
        Ok(())
    }

    async fn finalize_turn(&mut self, turn_errored: bool) -> anyhow::Result<()> {
        if turn_errored {
            if let Some(text) = self.pending_send.take() {
                let len = self.session.messages.len();
                if len > 0 && self.session.messages[len - 1].role == MessageRole::User {
                    self.session.truncate_to(len - 1);
                }
                self.input.buffer = text.into();
                self.input.cursor = self.input.buffer.len();
            }
        } else if !self.is_running {
            self.pending_send = None;
        }

        if !self.is_running
            && let Some(restore_name) = self.dot_prompt_restore.take()
        {
            self.context.current_prompt = self.context.prompts.get(&restore_name).cloned();
            self.context.current_prompt_name = if self.context.current_prompt.is_some() {
                Some(restore_name)
            } else {
                None
            };
            if let Some(perm) = &self.permission {
                let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                guard.restore_user_mode();
            }
        }

        if !self.is_running
            && self.chain_pending.is_none()
            && let Some(ref name) = self.context.current_prompt_name
            && !self.context.chain_declined.contains(name)
            && let Some(phase) = crate::extras::chain::ChainPhase::from_prompt_name(name)
            && let Some(ref chain_cfg) = self.cfg.chain
            && phase.is_enabled(chain_cfg)
        {
            self.chain_pending = Some(phase);
            self.chain_label_msg = Some(phase.chain_label().to_string());
            self.renderer.chain_but_mode = false;
            self.renderer.chain_prompt = Some(ChainPrompt {
                question: compact_str::CompactString::from(phase.chain_label()),
            });
        }

        if !self.is_running {
            self.main_abort = None;
            if let Some(next) = self.pending_inputs.pop_front() {
                self.renderer.chain_prompt = None;
                self.renderer.chain_but_mode = false;
                self.chain_pending = None;
                self.chain_label_msg = None;
                for line in next.lines() {
                    self.renderer
                        .write_line(&format!("> {}", sanitize_output(line)), Color::Green)?;
                }
                self.renderer.write_line("", Color::White)?;
                self.start_main_run(&next).await;
            }
        }

        Ok(())
    }

    fn abort_main_run(&mut self) -> anyhow::Result<()> {
        if let Some(h) = self.main_abort.take() {
            h.abort();
        }
        self.sandbox.kill_active();
        self.is_running = false;
        if let Some(ss) = self.status_signals.as_ref() {
            ss.send_stop();
        }
        self.agent_rx = None;
        self.turn_trace.clear();
        self.awaiting_compaction_relief = false;
        self.pending_inputs.clear();
        #[cfg(feature = "loop")]
        if let Some(ref mut ls) = self.loop_state {
            ls.active = false;
            self.loop_label = None;
        }
        if !self.input.buffer.is_empty() {
            self.input.clear_buffer();
        }
        if let Some(restore_name) = self.dot_prompt_restore.take() {
            self.context.current_prompt = self.context.prompts.get(&restore_name).cloned();
            self.context.current_prompt_name = if self.context.current_prompt.is_some() {
                Some(restore_name)
            } else {
                None
            };
            if let Some(perm) = &self.permission {
                let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                guard.restore_user_mode();
            }
        }
        self.renderer.write_line(
            "interrupted (changes may be partial; review with git diff)",
            C_ERROR,
        )?;
        Ok(())
    }

    async fn start_main_run(&mut self, text: &str) {
        start_main_run(
            text,
            &mut self.agent,
            &self.client,
            self.session,
            self.cli,
            self.cfg,
            self.context,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            self.reasoning_enabled,
            &mut self.agent_rx,
            &mut self.main_abort,
            &mut self.is_running,
            &self.status_signals,
            #[cfg(feature = "mcp")]
            &mut self.mcp_manager,
            &mut self.prebuild_rx,
            &mut self.pending_send,
        )
        .await;
    }

    async fn ensure_agent(&mut self) {
        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        event_handler::ensure_agent(
            &mut self.agent,
            &self.client,
            self.session,
            self.cli,
            self.cfg,
            self.context,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            self.reasoning_enabled,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await;
    }

    async fn run_chain_transition(
        &mut self,
        phase: crate::extras::chain::ChainPhase,
        extra: Option<&str>,
    ) -> anyhow::Result<()> {
        let next_name = phase.next_prompt_name();
        if let Some(content) = self.context.prompts.get(next_name).cloned() {
            let (mode_directive_str, clean_content) =
                crate::permission::parse_prompt_mode(&content);
            let mode_directive = mode_directive_str.map(|s| s.to_string());
            self.context.current_prompt = Some(if mode_directive.is_some() {
                clean_content.to_string()
            } else {
                content
            });
            self.context.current_prompt_name = Some(next_name.to_string());
            if let Some(ref mode_str) = mode_directive {
                if mode_str == "last_user_mode"
                    && let Some(perm) = &self.permission
                {
                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                    guard.restore_user_mode();
                } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str)
                    && let Some(perm) = &self.permission
                {
                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                    guard.set_prompt_mode(mode);
                }
            }
        }
        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        apply_prompt_model(
            next_name,
            self.cfg,
            self.cli,
            &mut self.client,
            self.session,
            &mut self.agent,
            self.context,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            self.reasoning_enabled,
            &mut self.renderer,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await;
        let base_msg = phase.transition_message().to_string();
        let msg = if let Some(extra) = extra {
            format!("{}\n\nAdditional instructions: {}", base_msg, extra)
        } else {
            base_msg
        };
        for line in msg.lines() {
            self.renderer
                .write_line(&format!("> {}", sanitize_output(line)), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;
        self.session.add_message(MessageRole::User, &msg);
        self.agent = None;
        self.start_main_run(&msg).await;
        Ok(())
    }

    async fn handle_dot_command(
        &mut self,
        text: &mut compact_str::CompactString,
    ) -> anyhow::Result<bool> {
        if !text.starts_with('.') {
            return Ok(false);
        }
        let after_dot = text[1..].trim_start();

        for line in text.lines() {
            let safe_line = sanitize_output(line);
            self.renderer
                .write_line(&format!("> {}", safe_line), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;

        if after_dot.is_empty() {
            self.input.buffer = ".".into();
            self.input.cursor = 1;
            self.input.start_dot_picker();
            return Ok(true);
        }

        if let Some((prompt_name, msg)) = after_dot.split_once(char::is_whitespace) {
            let prompt_name = prompt_name.trim();
            let msg = msg.trim();
            if !prompt_name.is_empty() && self.context.prompts.contains_key(prompt_name) {
                self.dot_prompt_restore = self.context.current_prompt_name.clone();
                if let Some(content) = self.context.prompts.get(prompt_name).cloned() {
                    self.apply_prompt_from_content(&content, prompt_name)
                        .await?;
                }
                #[cfg(feature = "mcp")]
                let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                apply_prompt_model(
                    prompt_name,
                    self.cfg,
                    self.cli,
                    &mut self.client,
                    self.session,
                    &mut self.agent,
                    self.context,
                    &self.permission,
                    &self.ask_tx,
                    &self.sandbox,
                    self.reasoning_enabled,
                    &mut self.renderer,
                    #[cfg(feature = "mcp")]
                    mcp_ref,
                )
                .await;
                *text = msg.to_string().into();
                self.agent = None;
                return Ok(false);
            } else {
                self.renderer
                    .write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
                return Ok(true);
            }
        }

        let prompt_name = after_dot.trim();
        if self.context.prompts.contains_key(prompt_name) {
            if let Some(content) = self.context.prompts.get(prompt_name).cloned() {
                self.apply_prompt_from_content(&content, prompt_name)
                    .await?;
            }
            #[cfg(feature = "mcp")]
            let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
            apply_prompt_model(
                prompt_name,
                self.cfg,
                self.cli,
                &mut self.client,
                self.session,
                &mut self.agent,
                self.context,
                &self.permission,
                &self.ask_tx,
                &self.sandbox,
                self.reasoning_enabled,
                &mut self.renderer,
                #[cfg(feature = "mcp")]
                mcp_ref,
            )
            .await;
            self.agent = None;
            self.renderer
                .write_line(&format!("switched to prompt '{}'", prompt_name), C_AGENT)?;
            self.save_session()?;
            Ok(true)
        } else {
            self.renderer
                .write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
            Ok(true)
        }
    }

    async fn apply_prompt_from_content(
        &mut self,
        content: &str,
        prompt_name: &str,
    ) -> anyhow::Result<()> {
        let (mode_directive_str, clean_content) = crate::permission::parse_prompt_mode(content);
        let mode_directive = mode_directive_str.map(|s| s.to_string());
        self.context.current_prompt = Some(if mode_directive.is_some() {
            clean_content.to_string()
        } else {
            content.to_string()
        });
        self.context.current_prompt_name = Some(prompt_name.to_string());
        if let Some(ref mode_str) = mode_directive
            && let Some(perm) = &self.permission
        {
            let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
            if mode_str == "last_user_mode" {
                guard.restore_user_mode();
            } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str) {
                guard.set_prompt_mode(mode);
            }
        }
        Ok(())
    }

    fn run_queue_command(&mut self, arg: &str) -> anyhow::Result<()> {
        match arg {
            "clear" => {
                let n = self.pending_inputs.len();
                self.pending_inputs.clear();
                self.renderer
                    .write_line(&format!("queue cleared ({} removed)", n), C_TOOL)?;
            }
            "pop" => match self.pending_inputs.pop_back() {
                Some(x) => self
                    .renderer
                    .write_line(&format!("unqueued: {}", sanitize_output(&x)), C_TOOL)?,
                None => self.renderer.write_line("queue is empty", C_TOOL)?,
            },
            "" | "ls" | "list" => {
                if self.pending_inputs.is_empty() {
                    self.renderer.write_line("queue is empty", C_TOOL)?;
                } else {
                    self.renderer
                        .write_line(&format!("queued ({}):", self.pending_inputs.len()), C_TOOL)?;
                    for (i, q) in self.pending_inputs.iter().enumerate() {
                        self.renderer
                            .write_line(&format!("  {}. {}", i + 1, sanitize_output(q)), C_TOOL)?;
                    }
                }
            }
            _ => self
                .renderer
                .write_line("usage: /queue [ls|clear|pop]", C_ERROR)?,
        }
        Ok(())
    }

    async fn run_btw(&mut self, text: &str) -> anyhow::Result<()> {
        for line in text.lines() {
            self.renderer
                .write_line(&format!("> {}", sanitize_output(line)), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;
        let btw_text = text
            .trim_start()
            .strip_prefix("/btw")
            .map(|s| s.trim())
            .unwrap_or("");
        if btw_text.is_empty() {
            self.renderer.write_line("usage: /btw <message>", C_AGENT)?;
            return Ok(());
        }
        let id = self.btw_next_id;
        self.btw_next_id = self.btw_next_id.wrapping_add(1);
        let snapshot = crate::agent::runner::build_btw_snapshot(
            self.session,
            &self.turn_trace,
            self.is_running,
        );
        let model = self.client.completion_model(self.session.model.to_string());
        let temperature =
            crate::config::resolve_temperature(self.cli, self.cfg, &self.session.model);
        let extra_body = crate::config::resolve_extra_body(self.cfg, &self.session.model);
        let btw_agent = crate::provider::build_btw_agent(
            model,
            self.cli,
            self.cfg,
            self.context,
            &self.permission,
            &self.ask_tx,
            self.reasoning_enabled,
            temperature,
            extra_body,
        );
        let runner = btw_agent.spawn_btw(
            btw_text.to_string(),
            snapshot,
            self.btw_tx.clone(),
            id,
            self.cfg.retry.clone(),
        );
        self.btw_abort.push((id, runner.abort_handle));
        self.btw_inflight += 1;
        self.renderer
            .write_line(&format!("[btw #{}] thinking...", id), C_BTW)?;
        Ok(())
    }

    async fn run_slash_command(&mut self, text: &str) -> anyhow::Result<()> {
        for line in text.lines() {
            let safe_line = sanitize_output(line);
            self.renderer
                .write_line(&format!("> {}", safe_line), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;

        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        let result = handle_slash(
            text,
            &mut self.agent,
            &mut self.client,
            &mut self.renderer,
            self.session,
            self.cli,
            self.cfg,
            self.context,
            &mut self.show_reasoning,
            &mut self.reasoning_enabled,
            &mut self.is_running,
            &mut self.input,
            &self.permission,
            &self.ask_tx,
            &mut self.todo_tools_enabled,
            &self.sandbox,
            #[cfg(feature = "loop")]
            &mut self.loop_state,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await;

        {
            let provider = self.session.provider.to_string();
            let is_custom = self.cfg.custom_providers_map().contains_key(&provider);
            let ids = crate::ui::slash::warm_model_cache(
                &provider,
                is_custom,
                &self.client,
                self.cli,
                self.cfg,
            )
            .await;
            self.input.set_live_model_names(ids);
        }

        self.handle_slash_result(result).await?;
        self.save_session()?;
        Ok(())
    }

    async fn handle_slash_result(&mut self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        match result {
            Err(e) if e.to_string().starts_with("DEFER_COMPRESS:") => {
                let err_msg = e.to_string();
                let instructions = err_msg.strip_prefix("DEFER_COMPRESS:").and_then(|s| {
                    let s = s.trim();
                    if s.is_empty() || s == "(none)" {
                        None
                    } else {
                        Some(s.to_string())
                    }
                });
                #[cfg(feature = "mcp")]
                let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                let compress_result = handle_compress(
                    instructions.as_deref(),
                    false,
                    &mut self.agent,
                    &mut self.client,
                    &mut self.renderer,
                    self.session,
                    self.cli,
                    self.cfg,
                    self.context,
                    self.reasoning_enabled,
                    &self.permission,
                    &self.ask_tx,
                    &self.sandbox,
                    #[cfg(feature = "mcp")]
                    mcp_ref,
                )
                .await;
                if let Err(e) = compress_result {
                    self.renderer
                        .write_line(&format!("compress error: {}", e), C_ERROR)?;
                }
                let _ = crate::session::storage::save_session(self.session);
            }
            #[cfg(feature = "mcp")]
            Err(e)
                if e.to_string()
                    .starts_with(crate::ui::slash::settings::DEFER_MCP_LOGIN) =>
            {
                let server = e
                    .to_string()
                    .strip_prefix(crate::ui::slash::settings::DEFER_MCP_LOGIN)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let resolved = self
                    .cfg
                    .mcp_servers
                    .as_ref()
                    .and_then(|m| m.get(&server))
                    .and_then(|s| {
                        if let crate::extras::mcp::config::McpServerConfig::Url {
                            url, oauth, ..
                        } = s
                        {
                            oauth
                                .as_ref()
                                .and_then(|o| o.settings())
                                .map(|set| (url.clone(), set))
                        } else {
                            None
                        }
                    });
                match resolved {
                    Some((url, settings)) => {
                        self.renderer.write_line(
                            &format!("starting OAuth login for '{}'...", server),
                            C_AGENT,
                        )?;
                        match crate::extras::mcp::oauth::begin_login(&server, &url, &settings).await
                        {
                            Ok(login) => {
                                copy_to_clipboard(&login.auth_url);
                                self.renderer.write_line(
                                    "open this URL to authorize (copied to clipboard):",
                                    C_AGENT,
                                )?;
                                self.renderer.write_line(&login.auth_url, Color::Cyan)?;
                                self.renderer.write_line(
                                    &format!(
                                        "waiting for authorization on 127.0.0.1:{} in the background...",
                                        settings.redirect_port()
                                    ),
                                    Color::DarkGrey,
                                )?;
                                let tx = self.user_tx.clone();
                                let sname = compact_str::CompactString::new(&server);
                                tokio::spawn(async move {
                                    let error = login
                                        .wait_for_callback(Duration::from_secs(180))
                                        .await
                                        .err()
                                        .map(|e| compact_str::CompactString::new(e.to_string()));
                                    let _ = tx
                                        .send(UserEvent::McpLoginDone {
                                            server: sname,
                                            error,
                                        })
                                        .await;
                                });
                            }
                            Err(err) => {
                                self.renderer.write_line(
                                    &format!("login setup failed for '{}': {}", server, err),
                                    C_ERROR,
                                )?;
                            }
                        }
                    }
                    None => {
                        self.renderer.write_line(
                            &format!(
                                "cannot start login for '{}' (not an OAuth URL server)",
                                server
                            ),
                            C_ERROR,
                        )?;
                    }
                }
            }
            #[cfg(feature = "git-worktree")]
            Err(e)
                if e.downcast_ref::<crate::extras::git_worktree::DeferredWorktreeAction>()
                    .is_some() =>
            {
                let action = e
                    .downcast_ref::<crate::extras::git_worktree::DeferredWorktreeAction>()
                    .unwrap();
                match action {
                    crate::extras::git_worktree::DeferredWorktreeAction::Merge {
                        branch,
                        target,
                        main_path,
                        wt_path,
                    } => {
                        let force_flag = self.cli.resolve_wt_force(self.cfg);
                        spawn_merge_agent(
                            branch,
                            target,
                            main_path,
                            wt_path,
                            force_flag,
                            self.session,
                            &mut self.agent,
                            &self.client,
                            self.cli,
                            self.cfg,
                            self.context,
                            &self.permission,
                            &self.ask_tx,
                            &self.sandbox,
                            self.reasoning_enabled,
                            &mut self.agent_rx,
                            &mut self.main_abort,
                            &mut self.is_running,
                            &self.status_signals,
                            &mut self.wt_return_path,
                            #[cfg(feature = "mcp")]
                            &mut self.mcp_manager,
                        )
                        .await;
                    }
                    crate::extras::git_worktree::DeferredWorktreeAction::Exit { main_path } => {
                        std::env::set_current_dir(main_path)
                            .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
                        self.session.working_dir = compact_str::CompactString::new(main_path);
                        self.context.reload();
                        apply_current_prompt_mode(self.context, &self.permission);
                        #[cfg(feature = "mcp")]
                        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                        let model = self.client.completion_model(self.session.model.to_string());
                        let temperature = crate::config::resolve_temperature(
                            self.cli,
                            self.cfg,
                            &self.session.model,
                        );
                        let extra_body =
                            crate::config::resolve_extra_body(self.cfg, &self.session.model);
                        self.agent = Some(
                            crate::provider::build_agent(
                                model,
                                self.cli,
                                self.cfg,
                                self.context,
                                self.permission.clone(),
                                self.ask_tx.clone(),
                                self.sandbox.clone(),
                                self.reasoning_enabled,
                                temperature,
                                extra_body,
                                #[cfg(feature = "mcp")]
                                mcp_ref,
                            )
                            .await,
                        );
                        render_session(
                            &mut self.renderer,
                            self.session,
                            self.cli,
                            self.cfg,
                            self.context,
                        )?;
                        self.renderer.write_line(
                            &format!("returned to main repo at {}", main_path),
                            C_AGENT,
                        )?;
                    }
                }
            }
            Err(e) if e.to_string().starts_with("DEFER_INIT:") => {
                let prompt = e
                    .to_string()
                    .strip_prefix("DEFER_INIT:")
                    .unwrap_or("")
                    .to_string();
                self.ensure_agent().await;
                let history = crate::agent::runner::convert_history(self.session);
                let runner = self
                    .agent
                    .as_ref()
                    .unwrap()
                    .clone()
                    .spawn_runner(
                        prompt,
                        history,
                        self.cfg.retry.clone(),
                        #[cfg(feature = "hooks")]
                        None,
                    )
                    .await;
                self.agent_rx = Some(runner.event_rx);
                self.main_abort = Some(runner.abort_handle);
                self.is_running = true;
                if let Some(ss) = self.status_signals.as_ref() {
                    ss.send_start();
                }
            }
            Err(e) if e.to_string().starts_with("DEFER_REVIEW:") => {
                let msg = e
                    .to_string()
                    .strip_prefix("DEFER_REVIEW:")
                    .unwrap_or("")
                    .to_string();
                self.dot_prompt_restore = self.context.one_shot_restore.take();
                self.session.add_message(MessageRole::User, &msg);
                self.ensure_agent().await;
                let history = crate::agent::runner::convert_history(self.session);
                let runner = self
                    .agent
                    .as_ref()
                    .unwrap()
                    .clone()
                    .spawn_runner(
                        msg,
                        history,
                        self.cfg.retry.clone(),
                        #[cfg(feature = "hooks")]
                        None,
                    )
                    .await;
                self.agent_rx = Some(runner.event_rx);
                self.main_abort = Some(runner.abort_handle);
                self.is_running = true;
                if let Some(ss) = self.status_signals.as_ref() {
                    ss.send_start();
                }
            }
            Err(e) if e.to_string().starts_with("DEFER_EDITOR:") => {
                let path = e
                    .to_string()
                    .strip_prefix("DEFER_EDITOR:")
                    .unwrap_or("")
                    .to_string();
                let editor = self
                    .cfg
                    .editor
                    .clone()
                    .or_else(|| std::env::var("EDITOR").ok())
                    .unwrap_or_else(|| "editor".to_string());
                let _ = crossterm::terminal::disable_raw_mode();
                let mut stdout = std::io::stdout();
                let _ = stdout.execute(crossterm::event::DisableMouseCapture);
                let _ = stdout.execute(crossterm::terminal::LeaveAlternateScreen);
                let _ = stdout.flush();
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!("{} \"$1\"", editor))
                    .arg("sh")
                    .arg(&path)
                    .status();
                let _ = stdout.execute(crossterm::terminal::EnterAlternateScreen);
                let _ = stdout.execute(crossterm::terminal::Clear(
                    crossterm::terminal::ClearType::All,
                ));
                let _ = stdout.execute(crossterm::event::EnableMouseCapture);
                let _ = crossterm::terminal::enable_raw_mode();
                render_session(
                    &mut self.renderer,
                    self.session,
                    self.cli,
                    self.cfg,
                    self.context,
                )?;
                self.renderer
                    .write_line(&format!("returned from editing {}", path), C_AGENT)?;
            }
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::Interrupted) =>
            {
                return Err(e);
            }
            Err(e) => {
                self.renderer
                    .write_line(&format!("error: {}", e), C_ERROR)?;
            }
            Ok(()) => {
                self.save_session()?;
                #[cfg(feature = "loop")]
                if self
                    .loop_state
                    .as_ref()
                    .is_some_and(|ls| ls.active && ls.iteration == 0 && !self.is_running)
                {
                    #[allow(unused_variables)]
                    let (prompt, label, active) = {
                        let ls = self.loop_state.as_mut().unwrap();
                        ls.iteration = 1;
                        (ls.build_prompt(), ls.iteration_label(), ls.active)
                    };
                    self.ensure_agent().await;
                    let runner = self
                        .agent
                        .as_ref()
                        .unwrap()
                        .clone()
                        .spawn_runner(
                            prompt,
                            Vec::new(),
                            self.cfg.retry.clone(),
                            #[cfg(feature = "hooks")]
                            Some(crate::extras::hooks::LoopInfo {
                                iteration: 1,
                                active,
                            }),
                        )
                        .await;
                    self.agent_rx = Some(runner.event_rx);
                    self.main_abort = Some(runner.abort_handle);
                    self.is_running = true;
                    self.loop_label = Some(label);
                }
            }
        }
        Ok(())
    }

    async fn run_bang_command(&mut self, text: &str) -> anyhow::Result<()> {
        let cmd = text.strip_prefix('!').map(|s| s.trim()).unwrap_or("");
        if cmd.is_empty() {
            self.renderer
                .write_line("error: empty command after '!'", C_ERROR)?;
            return Ok(());
        }
        for line in text.lines() {
            let safe_line = sanitize_output(line);
            self.renderer
                .write_line(&format!("> {}", safe_line), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;

        let cmd_owned = cmd.to_string();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(&cmd_owned)
                .output()
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))?
        .map_err(|e| anyhow::anyhow!("command error: {}", e))?;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        let result = result.trim().to_string();

        for line in result.lines() {
            let safe_line = sanitize_output(line);
            self.renderer.write_line(
                &safe_line,
                if output.status.success() {
                    C_AGENT
                } else {
                    C_ERROR
                },
            )?;
        }
        self.renderer.write_line("", Color::White)?;

        self.session.add_message(MessageRole::User, text);
        self.session.add_message(MessageRole::Assistant, &result);
        if !self.cli.no_session {
            let _ = crate::session::chat_history::append_entry(
                &crate::session::chat_history::ChatHistoryEntry {
                    content: text.to_string(),
                    timestamp: self.session.updated_at.clone(),
                },
            );
        }
        Ok(())
    }

    async fn mid_turn_compact(&mut self, pressure: f64) -> anyhow::Result<()> {
        mid_turn_compact_and_respawn(
            pressure,
            &mut self.renderer,
            &mut self.agent,
            &mut self.client,
            self.session,
            self.cli,
            self.cfg,
            self.context,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            self.reasoning_enabled,
            &mut self.agent_rx,
            &mut self.main_abort,
            &mut self.is_running,
            &self.status_signals,
            &mut self.turn_trace,
            &mut self.response_buf,
            &mut self.response_start_block,
            &mut self.agent_line_started,
            &mut self.was_reasoning,
            #[cfg(feature = "mcp")]
            &mut self.mcp_manager,
        )
        .await
    }

    fn stop_context_exhausted(&mut self, prompt_tokens: u64, threshold: f64) -> anyhow::Result<()> {
        stop_turn_context_exhausted(
            prompt_tokens,
            threshold,
            &mut self.renderer,
            self.session,
            self.cfg,
            &mut self.agent_rx,
            &mut self.main_abort,
            &mut self.is_running,
            &self.status_signals,
            &mut self.turn_trace,
            &mut self.response_buf,
            &mut self.response_start_block,
            &mut self.agent_line_started,
            &mut self.was_reasoning,
        )
    }

    fn handle_btw_event(&mut self, bev: BtwEvent) -> anyhow::Result<()> {
        match bev {
            BtwEvent::Done {
                id,
                response,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
            } => {
                self.btw_total_cost += crate::pricing::estimate_cost(
                    crate::pricing::billable_input_tokens(
                        self.cfg.is_anthropic_native(&self.session.provider),
                        input_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                    ),
                    output_tokens,
                    self.session.input_token_cost,
                    self.session.output_token_cost,
                );
                self.btw_total_in = self.btw_total_in.saturating_add(input_tokens);
                self.btw_total_out = self.btw_total_out.saturating_add(output_tokens);
                self.btw_abort.retain(|(i, _)| *i != id);
                self.btw_inflight = self.btw_inflight.saturating_sub(1);
                self.renderer
                    .write_line(&format!("[btw #{}] answer:", id), C_BTW)?;
                for line in response.lines() {
                    self.renderer.write_line(&sanitize_output(line), C_AGENT)?;
                }
                self.renderer.write_line("", Color::White)?;
            }
            BtwEvent::Error { id, message } => {
                self.btw_abort.retain(|(i, _)| *i != id);
                self.btw_inflight = self.btw_inflight.saturating_sub(1);
                self.renderer.write_line(
                    &format!("[btw #{}] error: {}", id, sanitize_output(&message)),
                    C_ERROR,
                )?;
            }
        }
        Ok(())
    }

    fn take_prebuild(&mut self, prebuilt: PrebuildPayload, notify: bool) -> io::Result<()> {
        #[cfg(feature = "mcp")]
        {
            let (built_agent, built_mcp) = prebuilt;
            self.agent = Some(built_agent);
            self.mcp_manager = built_mcp;
            if notify && let Some(m) = self.mcp_manager.as_mut() {
                for notice in m.take_notices() {
                    self.renderer.write_line(&notice, C_ERROR)?;
                }
            }
        }
        #[cfg(not(feature = "mcp"))]
        {
            let _ = notify;
            self.agent = Some(prebuilt);
        }
        self.prebuild_rx = None;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn handle_mcp_login_done(
        &mut self,
        server: compact_str::CompactString,
        error: Option<compact_str::CompactString>,
    ) -> anyhow::Result<()> {
        if let Some(err) = error {
            self.renderer
                .write_line(&format!("login failed for '{}': {}", server, err), C_ERROR)?;
        } else {
            let server = server.to_string();
            let server_cfg = self
                .cfg
                .mcp_servers
                .as_ref()
                .and_then(|m| m.get(&server).cloned());
            match (self.mcp_manager.as_mut(), server_cfg) {
                (Some(mgr), Some(scfg)) => match mgr.reconnect(&server, &scfg).await {
                    Ok(()) => {
                        let model = self.client.completion_model(self.session.model.to_string());
                        let temperature = crate::config::resolve_temperature(
                            self.cli,
                            self.cfg,
                            &self.session.model,
                        );
                        let extra_body =
                            crate::config::resolve_extra_body(self.cfg, &self.session.model);
                        self.agent = Some(
                            crate::provider::build_agent(
                                model,
                                self.cli,
                                self.cfg,
                                self.context,
                                self.permission.clone(),
                                self.ask_tx.clone(),
                                self.sandbox.clone(),
                                self.reasoning_enabled,
                                temperature,
                                extra_body,
                                self.mcp_manager.as_ref(),
                            )
                            .await,
                        );
                        self.renderer.write_line(
                            &format!("authorized and connected '{}'", server),
                            C_AGENT,
                        )?;
                    }
                    Err(err) => {
                        self.renderer.write_line(
                            &format!("authorized '{}' but reconnect failed: {}", server, err),
                            C_ERROR,
                        )?;
                    }
                },
                _ => {
                    self.renderer.write_line(
                        &format!("authorized '{}' (will connect on next start)", server),
                        C_AGENT,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn rebind_event_thread(&mut self) {
        if let Some(h) = self.event_handle.take() {
            self.running.store(false, Ordering::Relaxed);
            let _ = h.join();
        }
        self.running = Arc::new(AtomicBool::new(true));
        let (new_tx, new_rx) = mpsc::channel(64);
        self.user_tx = new_tx;
        self.user_rx = new_rx;
        self.event_handle = Some(spawn_event_thread(
            self.user_tx.clone(),
            self.running.clone(),
        ));
    }

    fn run_lazygit(&mut self) -> anyhow::Result<()> {
        if std::process::Command::new("lazygit")
            .arg("--version")
            .output()
            .is_err()
        {
            self.renderer.write_line(
                "warning: lazygit not found — install it (https://github.com/jesseduffield/lazygit)",
                C_ERROR,
            )?;
            return Ok(());
        }
        if let Some(h) = self.event_handle.take() {
            self.running.store(false, Ordering::Relaxed);
            let _ = h.join();
        }
        let _ = crossterm::terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(crossterm::event::DisableMouseCapture);
        let _ = stdout.execute(crossterm::terminal::LeaveAlternateScreen);
        let _ = stdout.flush();
        let _ = std::process::Command::new("lazygit").status();
        let _ = stdout.execute(crossterm::terminal::EnterAlternateScreen);
        let _ = stdout.execute(crossterm::terminal::Clear(
            crossterm::terminal::ClearType::All,
        ));
        let _ = stdout.execute(crossterm::event::EnableMouseCapture);
        let _ = crossterm::terminal::enable_raw_mode();
        self.rebind_event_thread();
        Ok(())
    }

    fn save_session(&mut self) -> anyhow::Result<()> {
        if !self.cli.no_session
            && let Err(e) = crate::session::storage::save_session(self.session)
        {
            self.renderer
                .write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
        }
        Ok(())
    }

    #[cfg(feature = "git-worktree")]
    async fn handle_worktree_auto_merge(&mut self) -> anyhow::Result<()> {
        if !self.cli.resolve_wt_auto_merge(self.cfg) {
            return Ok(());
        }
        let info = match crate::extras::git_worktree::detect() {
            Some(i) => i,
            None => return Ok(()),
        };
        let target = crate::extras::git_worktree::default_branch(&info.main_repo_path)
            .unwrap_or_else(|| "main".to_string());

        let _ = self.renderer.write_line(
            &format!(
                "auto-merging worktree '{}' into '{}'...",
                info.branch, target
            ),
            C_AGENT,
        );
        let mut proceed = true;
        if crate::extras::git_worktree::worktree_has_uncommitted(&info.worktree_path) {
            let _ = self.renderer.write_line(
                "worktree has uncommitted changes. [c]ommit all and continue  [a]bort merge",
                C_PERM,
            );
            if let Some(ss) = self.status_signals.as_ref() {
                ss.send_git_conflict();
            }
            let action = loop {
                tokio::select! {
                    Some(ev) = self.user_rx.recv() => {
                        if let UserEvent::Key(key) = ev {
                            match key.code {
                                KeyCode::Char('c') | KeyCode::Char('C') => break 'c',
                                KeyCode::Char('a') | KeyCode::Char('A') => break 'a',
                                KeyCode::Enter | KeyCode::Esc => break 'a',
                                _ => {}
                            }
                        }
                    }
                }
            };
            match action {
                'c' => {
                    if let Err(e) =
                        crate::extras::git_worktree::worktree_auto_commit_all(&info.worktree_path)
                    {
                        let _ = self
                            .renderer
                            .write_line(&format!("auto-commit failed: {}", e), C_ERROR);
                        proceed = false;
                    } else {
                        let _ = self.renderer.write_line(
                            "committed all worktree changes, proceeding with merge",
                            C_AGENT,
                        );
                    }
                }
                'a' => {
                    let _ = self
                        .renderer
                        .write_line("merge aborted, worktree left untouched", C_AGENT);
                    proceed = false;
                }
                _ => unreachable!(),
            }
        }
        let (state, outcome) = if proceed {
            crate::extras::git_worktree::try_merge(&info, &target)
        } else {
            (
                crate::extras::git_worktree::MergeState {
                    info: info.clone(),
                    original_branch: String::new(),
                    orig_dir: std::path::PathBuf::new(),
                    stashed: false,
                },
                crate::extras::git_worktree::MergeOutcome::Error("aborted by user".into()),
            )
        };
        match outcome {
            crate::extras::git_worktree::MergeOutcome::Success => {
                let merge_result = if self.cli.resolve_wt_force(self.cfg) {
                    crate::extras::git_worktree::complete_merge_force(&state)
                } else {
                    crate::extras::git_worktree::complete_merge(&state)
                };
                match merge_result {
                    Ok(()) => {
                        let _ = self.renderer.write_line(
                            &format!("merged '{}' into '{}' and cleaned up", info.branch, target),
                            C_AGENT,
                        );
                    }
                    Err(e) => {
                        let _ = self.renderer.write_line(
                            &format!("merge succeeded but cleanup failed: {}", e),
                            C_ERROR,
                        );
                    }
                }
            }
            crate::extras::git_worktree::MergeOutcome::Conflicts(files) => {
                let _ = self.renderer.write_line(
                    &format!("merge conflict in {} file(s):", files.len()),
                    C_ERROR,
                );
                for f in &files {
                    let _ = self.renderer.write_line(&format!("  {}", f), C_ERROR);
                }
                if let Some(ss) = self.status_signals.as_ref() {
                    ss.send_git_conflict();
                }
                let _ = self.renderer.write_line(
                    "[a]bort  [l]eave for manual resolution  [h]elp (agent resolves)",
                    C_PERM,
                );

                let action = loop {
                    tokio::select! {
                        Some(ev) = self.user_rx.recv() => {
                            if let UserEvent::Key(key) = ev {
                                match key.code {
                                    KeyCode::Char('a') | KeyCode::Char('A') => break 'a',
                                    KeyCode::Char('l') | KeyCode::Char('L') => break 'l',
                                    KeyCode::Char('h') | KeyCode::Char('H') => break 'h',
                                    KeyCode::Enter | KeyCode::Esc => break 'a',
                                    _ => {}
                                }
                            }
                        }
                    }
                };

                match action {
                    'a' => {
                        let _ = crate::extras::git_worktree::cancel_merge(&state);
                        crate::extras::git_worktree::cleanup_worktree(
                            &info.worktree_path.to_string_lossy(),
                            &info.branch,
                            &info.main_repo_path.to_string_lossy(),
                            self.cli.resolve_wt_force(self.cfg),
                        );
                        let _ = self
                            .renderer
                            .write_line("merge aborted, restored original state", C_AGENT);
                    }
                    'l' => {
                        let _ = self.renderer.write_line(
                            &format!(
                                "conflict state left in {} for manual resolution",
                                info.main_repo_path.display()
                            ),
                            C_AGENT,
                        );
                    }
                    'h' => {
                        let _ = crate::extras::git_worktree::cancel_merge(&state);
                        let _ = self
                            .renderer
                            .write_line("agent resolving merge...", C_AGENT);
                        let main_path = info.main_repo_path.display().to_string();
                        let wt_path = info.worktree_path.display().to_string();
                        let force_flag = self.cli.resolve_wt_force(self.cfg);
                        spawn_merge_agent(
                            &info.branch,
                            &target,
                            &main_path,
                            &wt_path,
                            force_flag,
                            self.session,
                            &mut self.agent,
                            &self.client,
                            self.cli,
                            self.cfg,
                            self.context,
                            &self.permission,
                            &self.ask_tx,
                            &self.sandbox,
                            self.reasoning_enabled,
                            &mut self.agent_rx,
                            &mut self.main_abort,
                            &mut self.is_running,
                            &self.status_signals,
                            &mut self.wt_return_path,
                            #[cfg(feature = "mcp")]
                            &mut self.mcp_manager,
                        )
                        .await;

                        let mut agent_line_started = false;
                        let mut merge_response_buf = String::new();
                        let mut merge_response_start_block = None;
                        let mut merge_was_reasoning = false;
                        while self.is_running {
                            let ev = match self.agent_rx.as_mut() {
                                Some(rx) => {
                                    tokio::select! {
                                        Some(e) = rx.recv() => Some(e),
                                        Some(ev) = self.user_rx.recv() => {
                                            if let UserEvent::Key(key) = ev {
                                                let is_ctrl_c = key.code == KeyCode::Char('c')
                                                    && key.modifiers.contains(KeyModifiers::CONTROL);
                                                if is_ctrl_c {
                                                    if let Some(h) = self.main_abort.take() {
                                                        h.abort();
                                                    }
                                                    self.sandbox.kill_active();
                                                    self.is_running = false;
                                                    if let Some(ss) = self.status_signals.as_ref() {
                                                        ss.send_stop();
                                                    }
                                                    self.agent_rx = None;
                                                }
                                            }
                                            None
                                        }
                                        Some(ask_req) = async {
                                            if let Some(rx) = self.ask_rx.as_mut() {
                                                rx.recv().await
                                            } else {
                                                std::future::pending().await
                                            }
                                        } => {
                                            let _ = handle_permission_request(
                                                ask_req,
                                                &mut self.renderer,
                                                self.session,
                                                self.cli,
                                                &mut self.user_rx,
                                                &mut agent_line_started,
                                                &mut merge_was_reasoning,
                                            ).await;
                                            None
                                        }
                                    }
                                }
                                None => break,
                            };
                            if let Some(ev) = ev {
                                #[cfg(feature = "mcp")]
                                let mcp_ref =
                                    ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                                event_handler::handle_agent_event(
                                    ev,
                                    &mut self.renderer,
                                    self.session,
                                    self.cfg,
                                    self.cli,
                                    self.context,
                                    &mut self.is_running,
                                    &mut self.agent_rx,
                                    &mut agent_line_started,
                                    &mut merge_response_buf,
                                    &mut merge_response_start_block,
                                    &mut merge_was_reasoning,
                                    self.show_reasoning,
                                    &mut self.agent,
                                    &mut self.client,
                                    &mut self.loop_label,
                                    &self.permission,
                                    &self.ask_tx,
                                    &self.sandbox,
                                    &self.status_signals,
                                    #[cfg(feature = "loop")]
                                    &mut self.loop_state,
                                    #[cfg(feature = "git-worktree")]
                                    &mut self.wt_return_path,
                                    #[cfg(feature = "mcp")]
                                    mcp_ref,
                                )
                                .await?;
                            }
                        }
                    }
                    _ => unreachable!(),
                }
            }
            crate::extras::git_worktree::MergeOutcome::Error(e) => {
                let _ = self
                    .renderer
                    .write_line(&format!("merge failed: {}", e), C_ERROR);
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "git-worktree"))]
    async fn handle_worktree_auto_merge(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
