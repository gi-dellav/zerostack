use std::io::{self, Write};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::config;
use crate::event::{AgentEvent, BtwEvent, UserEvent};
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::provider::AnyAgent;
use crate::session::MessageRole;
use crate::ui::event_handler;
use crate::ui::events::{render_session, sanitize_output};
use crate::ui::input::InputEditor;
use crate::ui::permission_handler::handle_permission_request;
use crate::ui::pickers::rewind::RewindOutcome;
use crate::ui::renderer::{self as renderer_mod, ChainPrompt, Renderer, copy_to_clipboard};
use crate::ui::slash::{apply_prompt_model, handle_compress, handle_slash};
#[cfg(feature = "git-worktree")]
use crate::ui::state::MergeRequest;
use crate::ui::state::{AgentRunState, BtwStats, ChainState, SlashState, UiContext};
use crate::ui::terminal::TerminalGuard;
use crate::ui::utils::{parse_color, to_ansi_256};

#[cfg(all(feature = "mcp", feature = "git-worktree"))]
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
    ui: UiContext<'a>,
    run: AgentRunState,
    chain: ChainState,
    slash: SlashState,

    renderer: Renderer,
    input: InputEditor,
    last_branch_check: std::time::Instant,
    ask_rx: Option<mpsc::Receiver<crate::permission::ask::AskRequest>>,
    #[cfg(feature = "advisor")]
    handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,

    btw_tx: mpsc::Sender<BtwEvent>,
    btw_rx: mpsc::Receiver<BtwEvent>,
    btw_abort: Vec<(u32, tokio::task::AbortHandle)>,
    btw_inflight: usize,
    btw_next_id: u32,
    btw_total_cost: f64,
    btw_total_in: u64,
    btw_total_out: u64,

    user_tx: mpsc::Sender<UserEvent>,
    user_rx: mpsc::Receiver<UserEvent>,
    running: Arc<AtomicBool>,
    event_handle: Option<std::thread::JoinHandle<()>>,
    prebuild_rx: Option<mpsc::Receiver<PrebuildPayload>>,
    _terminal_guard: TerminalGuard,
}

impl<'a> App<'a> {
    pub(crate) async fn new(
        mut ui: UiContext<'a>,
        agent: Option<AnyAgent>,
        ask_rx: Option<mpsc::Receiver<crate::permission::ask::AskRequest>>,
        auto_trigger_msg: Option<String>,
        #[cfg(feature = "advisor")] handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,
    ) -> anyhow::Result<Self> {
        let _terminal_guard = TerminalGuard::new()?;

        ui.session.show_cost_always = ui.cfg.resolve_show_cost_always();
        crate::ui::statusline::init(ui.cfg);

        ui.session.refresh_git_branch();
        if crate::ui::statusline::needs_git_status() {
            ui.session.refresh_git_status();
        }
        let last_branch_check = std::time::Instant::now();

        let mut renderer = Renderer::new()?;
        renderer.set_statusline_height(crate::ui::statusline::line_count());
        renderer.set_monochrome(ui.cli.no_color);
        renderer.set_chat_margin(ui.cfg.resolve_chat_left_margin());
        if let Some(ref theme_name) = ui.context.current_theme_name {
            if let Some(content) = ui.context.themes.get(theme_name.as_str()) {
                crate::context::themes::apply(content, &mut renderer);
            }
        } else if let Some(colors) = &ui.cfg.colors {
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
        input.set_monochrome(ui.cli.no_color);
        input.set_prompt_names(ui.context.prompts.keys().cloned().collect());
        input.set_theme_names(ui.context.themes.keys().cloned().collect());
        if let Some(editor) = &ui.cfg.editor {
            input.set_editor(editor.clone());
        }
        input.set_quick_model_names(config::quick_models_map(ui.cfg).into_keys().collect());
        {
            let mut providers: Vec<String> =
                ["anthropic", "openai", "gemini", "openrouter", "ollama"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            providers.extend(ui.cfg.custom_providers_map().keys().cloned());
            input.set_provider_names(providers);
        }
        input.load_global_history();

        let mut run = AgentRunState {
            agent,
            ..AgentRunState::default()
        };
        let chain = ChainState::default();
        let slash = SlashState {
            show_reasoning: ui.cfg.resolve_show_reasoning(),
            reasoning_enabled: true,
            todo_tools_enabled: false,
        };
        ui.session.reasoning_enabled = slash.reasoning_enabled;
        ui.session.overhead_tokens =
            crate::agent::builder::estimate_overhead(ui.context, slash.reasoning_enabled);

        render_session(&mut renderer, ui.session, ui.cli, ui.cfg, ui.context)?;
        let marker_path = crate::session::storage::data_dir().join("shown_welcome_msg");
        if ui.cfg.resolve_always_show_welcome() || !marker_path.exists() {
            crate::ui::events::show_welcome(&mut renderer)?;
            if !ui.cfg.resolve_always_show_welcome() {
                if let Some(dir) = marker_path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&marker_path, "");
            }
        }
        refresh_display(
            &mut renderer,
            &mut input,
            &ui,
            &run,
            &chain,
            BtwStats::default(),
        )?;

        {
            let provider = ui.session.provider.to_string();
            let is_custom = ui.cfg.custom_providers_map().contains_key(&provider);
            let ids = crate::ui::slash::warm_model_cache(
                &provider, is_custom, &ui.client, ui.cli, ui.cfg,
            )
            .await;
            input.set_live_model_names(ids);
        }

        #[cfg(feature = "git-worktree")]
        if let Some(name) = &ui.cli.worktree {
            let wt_base_dir = ui.cli.resolve_wt_base_dir(ui.cfg);
            match crate::extras::git_worktree::create(name, wt_base_dir.as_deref()) {
                Ok((path, _info)) => {
                    std::env::set_current_dir(&path).ok();
                    ui.session.working_dir =
                        compact_str::CompactString::new(path.to_string_lossy());
                    ui.context.reload();
                    apply_current_prompt_mode(ui.context, &ui.permission);
                    #[cfg(feature = "mcp")]
                    ensure_mcp_manager(&mut ui.mcp_manager, ui.cfg).await;
                    run.agent = Some(
                        ui.agent_build_ctx()
                            .rebuild_agent(&ui.session.model, slash.reasoning_enabled)
                            .await,
                    );
                    let _ = render_session(&mut renderer, ui.session, ui.cli, ui.cfg, ui.context);
                }
                Err(e) => {
                    let _ = renderer.write_line(&format!("worktree failed: {}", e), C_ERROR);
                }
            }
        }
        #[cfg(feature = "git-worktree")]
        if ui.cli.parallel {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let name = ts.to_string();
            let wt_base_dir = ui.cli.resolve_wt_base_dir(ui.cfg);
            match crate::extras::git_worktree::create(&name, wt_base_dir.as_deref()) {
                Ok((path, _info)) => {
                    std::env::set_current_dir(&path).ok();
                    ui.session.working_dir =
                        compact_str::CompactString::new(path.to_string_lossy());
                    ui.context.reload();
                    apply_current_prompt_mode(ui.context, &ui.permission);
                    #[cfg(feature = "mcp")]
                    ensure_mcp_manager(&mut ui.mcp_manager, ui.cfg).await;
                    run.agent = Some(
                        ui.agent_build_ctx()
                            .rebuild_agent(&ui.session.model, slash.reasoning_enabled)
                            .await,
                    );
                    let _ = render_session(&mut renderer, ui.session, ui.cli, ui.cfg, ui.context);
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

            event_handler::ensure_agent(&mut run.agent, &mut ui, slash.reasoning_enabled).await;
            let history = crate::agent::runner::convert_history(ui.session);
            let runner = run
                .agent
                .as_ref()
                .unwrap()
                .clone()
                .spawn_runner(
                    trigger_msg.to_string(),
                    history,
                    ui.cfg.retry.clone(),
                    #[cfg(feature = "hooks")]
                    None,
                )
                .await;
            run.agent_rx = Some(runner.event_rx);
            run.main_abort = Some(runner.abort_handle);
            run.is_running = true;
            if let Some(ss) = ui.status_signals.as_ref() {
                ss.send_start();
            }
            ui.session.add_message(MessageRole::User, trigger_msg);
            #[cfg(feature = "advisor")]
            crate::extras::advisor::set_session_messages(ui.session.messages.clone());
        }

        let (user_tx, user_rx) = mpsc::channel::<UserEvent>(64);
        let running = Arc::new(AtomicBool::new(true));
        let event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));

        let (prebuild_tx, prebuild_rx_raw) = mpsc::channel::<PrebuildPayload>(1);
        let prebuild_rx = Some(prebuild_rx_raw);
        if auto_trigger_msg.is_none() && run.agent.is_none() {
            let client_clone = ui.client.clone();
            let session_model = ui.session.model.to_string();
            let cli_clone = ui.cli.clone();
            let cfg_clone = ui.cfg.clone();
            let context_clone = ui.context.clone();
            let permission_clone = ui.permission.clone();
            let ask_tx_clone = ui.ask_tx.clone();
            let sandbox_clone = ui.sandbox.clone();
            let reasoning_enabled = slash.reasoning_enabled;
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

                let a = crate::ui::state::AgentBuildCtx {
                    cli: &cli_clone,
                    cfg: &cfg_clone,
                    context: &context_clone,
                    client: &client_clone,
                    permission: &permission_clone,
                    ask_tx: &ask_tx_clone,
                    sandbox: &sandbox_clone,
                    #[cfg(feature = "mcp")]
                    mcp_manager: mcp.as_ref(),
                }
                .rebuild_agent(&session_model, reasoning_enabled)
                .await;

                #[cfg(feature = "mcp")]
                let _ = prebuild_tx.send((a, mcp)).await;
                #[cfg(not(feature = "mcp"))]
                let _ = prebuild_tx.send(a).await;
            });
        }

        let (btw_tx, btw_rx) = mpsc::channel::<BtwEvent>(32);

        Ok(Self {
            ui,
            run,
            chain,
            slash,
            renderer,
            input,
            last_branch_check,
            ask_rx,
            #[cfg(feature = "advisor")]
            handoff_rx,
            btw_tx,
            btw_rx,
            btw_abort: Vec::new(),
            btw_inflight: 0,
            btw_next_id: 0,
            btw_total_cost: 0.0,
            btw_total_in: 0,
            btw_total_out: 0,
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
            self.ui.session.reasoning_enabled = self.slash.reasoning_enabled;
            if self.last_branch_check.elapsed() >= Duration::from_secs(1) {
                self.ui.session.refresh_git_branch();
                if crate::ui::statusline::needs_git_status() {
                    self.ui.session.refresh_git_status();
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
                Some(prebuilt) = async { self.prebuild_rx.as_mut()?.recv().await }, if self.run.agent.is_none() => {
                    self.take_prebuild(prebuilt, true)?;
                    self.refresh()?;
                }
                Some(event) = async { self.run.agent_rx.as_mut()?.recv().await } => {
                    self.handle_agent_event(event).await?;
                }
                Some(ask_req) = async { self.ask_rx.as_mut()?.recv().await } => {
                    handle_permission_request(
                        ask_req,
                        &mut self.renderer,
                        &mut self.ui,
                        &mut self.run,
                        &mut self.user_rx,
                    ).await?;
                    self.refresh()?;
                }
                Some(bev) = self.btw_rx.recv() => {
                    self.handle_btw_event(bev)?;
                    self.refresh()?;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)), if self.run.is_running => {
                    self.refresh()?;
                }
                else => {
                    if let Some(rx) = self.prebuild_rx.as_mut()
                        && self.run.agent.is_none()
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
                handle_human_handoff(req, &mut self.renderer, &mut self.user_rx, &mut self.run)
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
        if let Some(mgr) = self.ui.mcp_manager {
            mgr.shutdown().await;
        }
    }

    fn refresh(&mut self) -> io::Result<()> {
        refresh_display(
            &mut self.renderer,
            &mut self.input,
            &self.ui,
            &self.run,
            &self.chain,
            BtwStats {
                cost: self.btw_total_cost,
                input: self.btw_total_in,
                output: self.btw_total_out,
            },
        )
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
                    } else if self.run.is_running {
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
            self.slash.show_reasoning = !self.slash.show_reasoning;
            self.renderer.write_line(
                &format!(
                    "reasoning visibility: {}",
                    if self.slash.show_reasoning {
                        "on"
                    } else {
                        "off"
                    }
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
                    .ui
                    .session
                    .messages
                    .get(idx)
                    .map(|m| m.content.to_string());
                if self.ui.session.rewind_to(idx) > 0 {
                    if let Some(text) = text {
                        self.input.load_text(&text);
                    }
                    if !self.ui.cli.no_session {
                        let _ = crate::session::storage::save_session(self.ui.session);
                    }
                    render_session(
                        &mut self.renderer,
                        self.ui.session,
                        self.ui.cli,
                        self.ui.cfg,
                        self.ui.context,
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
                    if let Some(phase) = self.chain.pending.take() {
                        self.chain.label_msg = None;
                        self.run_chain_transition(phase, None).await?;
                    }
                    return Ok(());
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.renderer.chain_prompt = None;
                    self.chain.pending = None;
                    self.chain.label_msg = None;
                    self.renderer
                        .write_line("chain declined — won't ask again this session", C_AGENT)?;
                    if let Some(ref name) = self.ui.context.current_prompt_name
                        && !self.ui.context.chain_declined.contains(name)
                    {
                        self.ui.context.chain_declined.push(name.clone());
                    }
                    return Ok(());
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    self.renderer.chain_but_mode = true;
                    self.renderer.chain_prompt = None;
                    self.input.clear_buffer();
                    self.chain.label_msg = self.chain.pending.map(|p| p.chain_label().to_string());
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
            if let Some(phase) = self.chain.pending {
                self.renderer.chain_prompt = Some(ChainPrompt {
                    question: compact_str::CompactString::from(phase.chain_label()),
                });
                self.chain.label_msg = Some(phase.chain_label().to_string());
            }
            self.input.clear_buffer();
            return Ok(());
        }

        if let Some(mut text) = self.input.handle_key(key) {
            #[cfg(feature = "loop")]
            if self.chain.loop_state.as_ref().is_some_and(|ls| ls.active) && !text.starts_with('/')
            {
                self.renderer
                    .write_line("loop active: /loop stop to cancel", C_ERROR)?;
                return Ok(());
            }
            if self.renderer.is_scrolling() {
                self.renderer.scroll_to_bottom()?;
            }

            // Chain-of-prompts: handle text submission after B (but) mode
            if !self.run.is_running
                && let Some(phase) = self.chain.pending.take()
            {
                self.chain.label_msg = None;
                self.renderer.chain_but_mode = false;
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    self.chain.pending = Some(phase);
                    self.chain.label_msg = Some(phase.chain_label().to_string());
                    self.renderer.chain_prompt = Some(ChainPrompt {
                        question: compact_str::CompactString::from(phase.chain_label()),
                    });
                    return Ok(());
                }
                self.run_chain_transition(phase, Some(&trimmed)).await?;
                return Ok(());
            }

            match classify_submission(self.run.is_running, &text) {
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
                    self.run.pending_inputs.push_back(text.to_string());
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
                if self.run.turn_trace.len() < TURN_TRACE_MAX {
                    self.run
                        .turn_trace
                        .push(compact_str::CompactString::from(format!(
                            "→ {}",
                            crate::ui::utils::format_tool_call_summary(name, args)
                        )));
                }
            }
            AgentEvent::ToolResult { output, .. } => {
                if self.run.turn_trace.len() < TURN_TRACE_MAX {
                    self.run
                        .turn_trace
                        .push(compact_str::CompactString::from(format!(
                            "← {}",
                            crate::extras::truncate::truncate_cjk(output, 500, "…")
                        )));
                }
            }
            AgentEvent::Done { .. } | AgentEvent::Error(_) => {
                self.run.turn_trace.clear();
                self.run.awaiting_compaction_relief = false;
            }
            _ => {}
        }

        #[cfg(feature = "loop")]
        let loop_running = self.chain.loop_state.as_ref().is_some_and(|ls| ls.active);
        #[cfg(not(feature = "loop"))]
        let loop_running = false;

        if let AgentEvent::CompletionCall {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            ..
        } = &event
            && self.run.is_running
            && !loop_running
            && !self.ui.cli.no_session
            && self.ui.cfg.resolve_compact_enabled()
            && self.ui.session.context_window > 0
            && let Some(threshold) = self.ui.cfg.resolve_mid_turn_compact_threshold()
        {
            let real_input_tokens = crate::session::Session::real_input_tokens(
                self.ui.cfg.is_anthropic_native(&self.ui.session.provider),
                *input_tokens,
                *cached_input_tokens,
                *cache_creation_input_tokens,
            );
            let pressure = real_input_tokens as f64 / self.ui.session.context_window as f64;
            if pressure > threshold {
                if self.run.awaiting_compaction_relief {
                    self.stop_context_exhausted(real_input_tokens, threshold)?;
                    self.run.awaiting_compaction_relief = false;
                } else {
                    self.mid_turn_compact(pressure).await?;
                    self.run.awaiting_compaction_relief = true;
                }
                self.refresh()?;
                return Ok(());
            } else {
                self.run.awaiting_compaction_relief = false;
            }
        }

        let turn_errored = matches!(&event, AgentEvent::Error(_));
        event_handler::handle_agent_event(
            event,
            &mut self.renderer,
            &mut self.run,
            &mut self.ui,
            &self.slash,
            &mut self.chain,
        )
        .await?;

        self.finalize_turn(turn_errored).await?;
        Ok(())
    }

    async fn finalize_turn(&mut self, turn_errored: bool) -> anyhow::Result<()> {
        if turn_errored {
            if let Some(text) = self.run.pending_send.take() {
                let len = self.ui.session.messages.len();
                if len > 0 && self.ui.session.messages[len - 1].role == MessageRole::User {
                    self.ui.session.truncate_to(len - 1);
                }
                self.input.buffer = text.into();
                self.input.cursor = self.input.buffer.len();
            }
        } else if !self.run.is_running {
            self.run.pending_send = None;
        }

        if !self.run.is_running
            && let Some(restore_name) = self.chain.dot_prompt_restore.take()
        {
            self.ui.context.current_prompt = self.ui.context.prompts.get(&restore_name).cloned();
            self.ui.context.current_prompt_name = if self.ui.context.current_prompt.is_some() {
                Some(restore_name)
            } else {
                None
            };
            if let Some(perm) = &self.ui.permission {
                let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                guard.restore_user_mode();
            }
        }

        if !self.run.is_running
            && self.chain.pending.is_none()
            && let Some(ref name) = self.ui.context.current_prompt_name
            && !self.ui.context.chain_declined.contains(name)
            && let Some(phase) = crate::extras::chain::ChainPhase::from_prompt_name(name)
            && let Some(ref chain_cfg) = self.ui.cfg.chain
            && phase.is_enabled(chain_cfg)
        {
            self.chain.pending = Some(phase);
            self.chain.label_msg = Some(phase.chain_label().to_string());
            self.renderer.chain_but_mode = false;
            self.renderer.chain_prompt = Some(ChainPrompt {
                question: compact_str::CompactString::from(phase.chain_label()),
            });
        }

        if !self.run.is_running {
            self.run.main_abort = None;
            if let Some(next) = self.run.pending_inputs.pop_front() {
                self.renderer.chain_prompt = None;
                self.renderer.chain_but_mode = false;
                self.chain.pending = None;
                self.chain.label_msg = None;
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
        if let Some(h) = self.run.main_abort.take() {
            h.abort();
        }
        self.ui.sandbox.kill_active();
        self.run.is_running = false;
        if let Some(ss) = self.ui.status_signals.as_ref() {
            ss.send_stop();
        }
        self.run.agent_rx = None;
        self.run.turn_trace.clear();
        self.run.awaiting_compaction_relief = false;
        self.run.pending_inputs.clear();
        #[cfg(feature = "loop")]
        if let Some(ref mut ls) = self.chain.loop_state {
            ls.active = false;
            self.chain.loop_label = None;
        }
        if !self.input.buffer.is_empty() {
            self.input.clear_buffer();
        }
        if let Some(restore_name) = self.chain.dot_prompt_restore.take() {
            self.ui.context.current_prompt = self.ui.context.prompts.get(&restore_name).cloned();
            self.ui.context.current_prompt_name = if self.ui.context.current_prompt.is_some() {
                Some(restore_name)
            } else {
                None
            };
            if let Some(perm) = &self.ui.permission {
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
            &mut self.run,
            &mut self.ui,
            &self.slash,
            &mut self.prebuild_rx,
        )
        .await;
    }

    async fn ensure_agent(&mut self) {
        event_handler::ensure_agent(
            &mut self.run.agent,
            &mut self.ui,
            self.slash.reasoning_enabled,
        )
        .await;
    }

    async fn run_chain_transition(
        &mut self,
        phase: crate::extras::chain::ChainPhase,
        extra: Option<&str>,
    ) -> anyhow::Result<()> {
        let next_name = phase.next_prompt_name();
        if let Some(content) = self.ui.context.prompts.get(next_name).cloned() {
            let (mode_directive_str, clean_content) =
                crate::permission::parse_prompt_mode(&content);
            let mode_directive = mode_directive_str.map(|s| s.to_string());
            self.ui.context.current_prompt = Some(if mode_directive.is_some() {
                clean_content.to_string()
            } else {
                content
            });
            self.ui.context.current_prompt_name = Some(next_name.to_string());
            if let Some(ref mode_str) = mode_directive {
                if mode_str == "last_user_mode"
                    && let Some(perm) = &self.ui.permission
                {
                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                    guard.restore_user_mode();
                } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str)
                    && let Some(perm) = &self.ui.permission
                {
                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                    guard.set_prompt_mode(mode);
                }
            }
        }
        apply_prompt_model(
            next_name,
            &mut self.ui,
            &mut self.run.agent,
            self.slash.reasoning_enabled,
            &mut self.renderer,
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
        self.ui.session.add_message(MessageRole::User, &msg);
        self.run.agent = None;
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
            if !prompt_name.is_empty() && self.ui.context.prompts.contains_key(prompt_name) {
                self.chain.dot_prompt_restore = self.ui.context.current_prompt_name.clone();
                if let Some(content) = self.ui.context.prompts.get(prompt_name).cloned() {
                    self.apply_prompt_from_content(&content, prompt_name)
                        .await?;
                }
                apply_prompt_model(
                    prompt_name,
                    &mut self.ui,
                    &mut self.run.agent,
                    self.slash.reasoning_enabled,
                    &mut self.renderer,
                )
                .await;
                *text = msg.to_string().into();
                self.run.agent = None;
                return Ok(false);
            } else {
                self.renderer
                    .write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
                return Ok(true);
            }
        }

        let prompt_name = after_dot.trim();
        if self.ui.context.prompts.contains_key(prompt_name) {
            if let Some(content) = self.ui.context.prompts.get(prompt_name).cloned() {
                self.apply_prompt_from_content(&content, prompt_name)
                    .await?;
            }
            apply_prompt_model(
                prompt_name,
                &mut self.ui,
                &mut self.run.agent,
                self.slash.reasoning_enabled,
                &mut self.renderer,
            )
            .await;
            self.run.agent = None;
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
        self.ui.context.current_prompt = Some(if mode_directive.is_some() {
            clean_content.to_string()
        } else {
            content.to_string()
        });
        self.ui.context.current_prompt_name = Some(prompt_name.to_string());
        if let Some(ref mode_str) = mode_directive
            && let Some(perm) = &self.ui.permission
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
                let n = self.run.pending_inputs.len();
                self.run.pending_inputs.clear();
                self.renderer
                    .write_line(&format!("queue cleared ({} removed)", n), C_TOOL)?;
            }
            "pop" => match self.run.pending_inputs.pop_back() {
                Some(x) => self
                    .renderer
                    .write_line(&format!("unqueued: {}", sanitize_output(&x)), C_TOOL)?,
                None => self.renderer.write_line("queue is empty", C_TOOL)?,
            },
            "" | "ls" | "list" => {
                if self.run.pending_inputs.is_empty() {
                    self.renderer.write_line("queue is empty", C_TOOL)?;
                } else {
                    self.renderer.write_line(
                        &format!("queued ({}):", self.run.pending_inputs.len()),
                        C_TOOL,
                    )?;
                    for (i, q) in self.run.pending_inputs.iter().enumerate() {
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
            self.ui.session,
            &self.run.turn_trace,
            self.run.is_running,
        );
        let model = self
            .ui
            .client
            .completion_model(self.ui.session.model.to_string());
        let temperature =
            crate::config::resolve_temperature(self.ui.cli, self.ui.cfg, &self.ui.session.model);
        let extra_body = crate::config::resolve_extra_body(self.ui.cfg, &self.ui.session.model);
        let btw_agent = crate::provider::build_btw_agent(
            model,
            self.ui.cli,
            self.ui.cfg,
            self.ui.context,
            &self.ui.permission,
            &self.ui.ask_tx,
            self.slash.reasoning_enabled,
            temperature,
            extra_body,
        );
        let runner = btw_agent.spawn_btw(
            btw_text.to_string(),
            snapshot,
            self.btw_tx.clone(),
            id,
            self.ui.cfg.retry.clone(),
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

        let result = handle_slash(
            text,
            &mut self.renderer,
            &mut self.input,
            &mut self.run,
            &mut self.ui,
            &mut self.slash,
            &mut self.chain,
        )
        .await;

        {
            let provider = self.ui.session.provider.to_string();
            let is_custom = self.ui.cfg.custom_providers_map().contains_key(&provider);
            let ids = crate::ui::slash::warm_model_cache(
                &provider,
                is_custom,
                &self.ui.client,
                self.ui.cli,
                self.ui.cfg,
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
                let compress_result = handle_compress(
                    instructions.as_deref(),
                    false,
                    &mut self.run.agent,
                    &mut self.renderer,
                    &mut self.ui,
                    self.slash.reasoning_enabled,
                )
                .await;
                if let Err(e) = compress_result {
                    self.renderer
                        .write_line(&format!("compress error: {}", e), C_ERROR)?;
                }
                let _ = crate::session::storage::save_session(self.ui.session);
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
                    .ui
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
                        let force_flag = self.ui.cli.resolve_wt_force(self.ui.cfg);
                        spawn_merge_agent(
                            MergeRequest {
                                branch,
                                target,
                                main_path,
                                wt_path,
                                force: force_flag,
                            },
                            &mut self.run,
                            &mut self.ui,
                            &mut self.chain,
                        )
                        .await;
                    }
                    crate::extras::git_worktree::DeferredWorktreeAction::Exit { main_path } => {
                        std::env::set_current_dir(main_path)
                            .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
                        self.ui.session.working_dir = compact_str::CompactString::new(main_path);
                        self.ui.context.reload();
                        apply_current_prompt_mode(self.ui.context, &self.ui.permission);
                        #[cfg(feature = "mcp")]
                        ensure_mcp_manager(&mut self.ui.mcp_manager, self.ui.cfg).await;
                        let new_agent = self
                            .ui
                            .agent_build_ctx()
                            .rebuild_agent(&self.ui.session.model, self.slash.reasoning_enabled)
                            .await;
                        self.run.agent = Some(new_agent);
                        render_session(
                            &mut self.renderer,
                            self.ui.session,
                            self.ui.cli,
                            self.ui.cfg,
                            self.ui.context,
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
                let history = crate::agent::runner::convert_history(self.ui.session);
                let runner = self
                    .run
                    .agent
                    .as_ref()
                    .unwrap()
                    .clone()
                    .spawn_runner(
                        prompt,
                        history,
                        self.ui.cfg.retry.clone(),
                        #[cfg(feature = "hooks")]
                        None,
                    )
                    .await;
                self.run.agent_rx = Some(runner.event_rx);
                self.run.main_abort = Some(runner.abort_handle);
                self.run.is_running = true;
                if let Some(ss) = self.ui.status_signals.as_ref() {
                    ss.send_start();
                }
            }
            Err(e) if e.to_string().starts_with("DEFER_REVIEW:") => {
                let msg = e
                    .to_string()
                    .strip_prefix("DEFER_REVIEW:")
                    .unwrap_or("")
                    .to_string();
                self.chain.dot_prompt_restore = self.ui.context.one_shot_restore.take();
                self.ui.session.add_message(MessageRole::User, &msg);
                self.ensure_agent().await;
                let history = crate::agent::runner::convert_history(self.ui.session);
                let runner = self
                    .run
                    .agent
                    .as_ref()
                    .unwrap()
                    .clone()
                    .spawn_runner(
                        msg,
                        history,
                        self.ui.cfg.retry.clone(),
                        #[cfg(feature = "hooks")]
                        None,
                    )
                    .await;
                self.run.agent_rx = Some(runner.event_rx);
                self.run.main_abort = Some(runner.abort_handle);
                self.run.is_running = true;
                if let Some(ss) = self.ui.status_signals.as_ref() {
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
                    .ui
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
                    self.ui.session,
                    self.ui.cli,
                    self.ui.cfg,
                    self.ui.context,
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
                    .chain
                    .loop_state
                    .as_ref()
                    .is_some_and(|ls| ls.active && ls.iteration == 0 && !self.run.is_running)
                {
                    #[allow(unused_variables)]
                    let (prompt, label, active) = {
                        let ls = self.chain.loop_state.as_mut().unwrap();
                        ls.iteration = 1;
                        (ls.build_prompt(), ls.iteration_label(), ls.active)
                    };
                    self.ensure_agent().await;
                    let runner = self
                        .run
                        .agent
                        .as_ref()
                        .unwrap()
                        .clone()
                        .spawn_runner(
                            prompt,
                            Vec::new(),
                            self.ui.cfg.retry.clone(),
                            #[cfg(feature = "hooks")]
                            Some(crate::extras::hooks::LoopInfo {
                                iteration: 1,
                                active,
                            }),
                        )
                        .await;
                    self.run.agent_rx = Some(runner.event_rx);
                    self.run.main_abort = Some(runner.abort_handle);
                    self.run.is_running = true;
                    self.chain.loop_label = Some(label);
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

        self.ui.session.add_message(MessageRole::User, text);
        self.ui.session.add_message(MessageRole::Assistant, &result);
        if !self.ui.cli.no_session {
            let _ = crate::session::chat_history::append_entry(
                &crate::session::chat_history::ChatHistoryEntry {
                    content: text.to_string(),
                    timestamp: self.ui.session.updated_at.clone(),
                },
            );
        }
        Ok(())
    }

    async fn mid_turn_compact(&mut self, pressure: f64) -> anyhow::Result<()> {
        mid_turn_compact_and_respawn(
            pressure,
            &mut self.renderer,
            &mut self.run,
            &mut self.ui,
            &self.slash,
        )
        .await
    }

    fn stop_context_exhausted(&mut self, prompt_tokens: u64, threshold: f64) -> anyhow::Result<()> {
        stop_turn_context_exhausted(
            prompt_tokens,
            threshold,
            &mut self.renderer,
            &self.ui,
            &mut self.run,
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
                        self.ui.cfg.is_anthropic_native(&self.ui.session.provider),
                        input_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                    ),
                    output_tokens,
                    self.ui.session.input_token_cost,
                    self.ui.session.output_token_cost,
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
            self.run.agent = Some(built_agent);
            self.ui.mcp_manager = built_mcp;
            if notify && let Some(m) = self.ui.mcp_manager.as_mut() {
                for notice in m.take_notices() {
                    self.renderer.write_line(&notice, C_ERROR)?;
                }
            }
        }
        #[cfg(not(feature = "mcp"))]
        {
            let _ = notify;
            self.run.agent = Some(prebuilt);
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
                .ui
                .cfg
                .mcp_servers
                .as_ref()
                .and_then(|m| m.get(&server).cloned());
            match (self.ui.mcp_manager.as_mut(), server_cfg) {
                (Some(mgr), Some(scfg)) => match mgr.reconnect(&server, &scfg).await {
                    Ok(()) => {
                        let new_agent = self
                            .ui
                            .agent_build_ctx()
                            .rebuild_agent(&self.ui.session.model, self.slash.reasoning_enabled)
                            .await;
                        self.run.agent = Some(new_agent);
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
        if !self.ui.cli.no_session
            && let Err(e) = crate::session::storage::save_session(self.ui.session)
        {
            self.renderer
                .write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
        }
        Ok(())
    }

    #[cfg(feature = "git-worktree")]
    async fn handle_worktree_auto_merge(&mut self) -> anyhow::Result<()> {
        if !self.ui.cli.resolve_wt_auto_merge(self.ui.cfg) {
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
            if let Some(ss) = self.ui.status_signals.as_ref() {
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
                let merge_result = if self.ui.cli.resolve_wt_force(self.ui.cfg) {
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
                if let Some(ss) = self.ui.status_signals.as_ref() {
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
                            self.ui.cli.resolve_wt_force(self.ui.cfg),
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
                        let force_flag = self.ui.cli.resolve_wt_force(self.ui.cfg);
                        spawn_merge_agent(
                            MergeRequest {
                                branch: &info.branch,
                                target: &target,
                                main_path: &main_path,
                                wt_path: &wt_path,
                                force: force_flag,
                            },
                            &mut self.run,
                            &mut self.ui,
                            &mut self.chain,
                        )
                        .await;

                        // The merge agent streams through the main run state; reset
                        // the streaming scratch so no stale partial line leaks in.
                        self.run.agent_line_started = false;
                        self.run.response_buf.clear();
                        self.run.response_start_block = None;
                        self.run.was_reasoning = false;
                        let mut merge_rx = self.run.agent_rx.take();
                        while self.run.is_running {
                            let ev = match merge_rx.as_mut() {
                                Some(rx) => {
                                    tokio::select! {
                                        Some(e) = rx.recv() => Some(e),
                                        Some(ev) = self.user_rx.recv() => {
                                            if let UserEvent::Key(key) = ev {
                                                let is_ctrl_c = key.code == KeyCode::Char('c')
                                                    && key.modifiers.contains(KeyModifiers::CONTROL);
                                                if is_ctrl_c {
                                                    if let Some(h) = self.run.main_abort.take() {
                                                        h.abort();
                                                    }
                                                    self.ui.sandbox.kill_active();
                                                    self.run.is_running = false;
                                                    if let Some(ss) = self.ui.status_signals.as_ref() {
                                                        ss.send_stop();
                                                    }
                                                    self.run.agent_rx = None;
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
                                                &mut self.ui,
                                                &mut self.run,
                                                &mut self.user_rx,
                                            ).await;
                                            None
                                        }
                                    }
                                }
                                None => break,
                            };
                            if let Some(ev) = ev {
                                event_handler::handle_agent_event(
                                    ev,
                                    &mut self.renderer,
                                    &mut self.run,
                                    &mut self.ui,
                                    &self.slash,
                                    &mut self.chain,
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
