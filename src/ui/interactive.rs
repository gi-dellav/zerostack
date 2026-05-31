use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::{AgentEvent, UserEvent};
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::permission::ask::{AskReceiver, AskSender};
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::dot_cmd::{apply_current_prompt_mode, report_prompt_switch, switch_prompt};
#[cfg(feature = "mcp")]
use crate::ui::event_handler::ensure_mcp_manager;
use crate::ui::event_handler::{ensure_agent, handle_agent_event};
use crate::ui::events::{render_session, sanitize_output, spawn_event_thread};
use crate::ui::input::InputEditor;
use crate::ui::permission_handler::handle_permission_request;
use crate::ui::renderer::{Renderer, copy_to_clipboard};
use crate::ui::slash::{handle_compress, handle_slash};
use crate::ui::status::StatusLine;
use crate::ui::terminal::TerminalGuard;
use crate::ui::utils::parse_color;
use compact_str::CompactString;

use super::{C_AGENT, C_ERROR, C_TOOL};

pub(crate) struct InteractiveSession<'a> {
    client: AnyClient,
    agent: Option<AnyAgent>,
    cli: &'a Cli,
    cfg: &'a Config,
    session: &'a mut Session,
    context: &'a mut ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    ask_rx: Option<AskReceiver>,
    sandbox: Sandbox,

    renderer: Renderer,
    input: InputEditor,
    user_tx: mpsc::Sender<UserEvent>,
    user_rx: mpsc::Receiver<UserEvent>,
    running: Arc<AtomicBool>,
    event_handle: Option<std::thread::JoinHandle<()>>,

    is_running: bool,
    agent_rx: Option<mpsc::Receiver<AgentEvent>>,
    subagent_rx: Option<mpsc::Receiver<AgentEvent>>,
    agent_line_started: bool,
    response_buf: String,
    response_start_line: Option<usize>,
    show_reasoning: bool,
    reasoning_enabled: bool,
    was_reasoning: bool,
    todo_tools_enabled: bool,
    loop_label: Option<String>,

    btw_active: bool,
    btw_msg_count: usize,
    btw_input_tokens: u64,
    btw_output_tokens: u64,
    btw_cost: f64,
    dot_prompt_restore: Option<String>,

    #[cfg(feature = "loop")]
    loop_state: Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "git-worktree")]
    wt_return_path: Option<String>,
    #[cfg(feature = "mcp")]
    mcp_manager: Option<McpClientManager>,
}

impl<'a> InteractiveSession<'a> {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        client: AnyClient,
        agent: Option<AnyAgent>,
        cli: &'a Cli,
        cfg: &'a Config,
        session: &'a mut Session,
        context: &'a mut ContextFiles,
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        ask_rx: Option<AskReceiver>,
        sandbox: Sandbox,
    ) -> anyhow::Result<InteractiveSession<'a>> {
        let _guard = TerminalGuard::new()?;

        #[cfg(feature = "mcp")]
        let mcp_manager: Option<McpClientManager> = None;

        let mut renderer = Renderer::new()?;
        renderer.set_monochrome(cli.no_color);
        if let Some(ref theme_name) = context.current_theme_name {
            if let Some(content) = context.themes.get(theme_name.as_str()) {
                crate::context::themes::apply(content, &mut renderer);
            }
        } else if let Some(colors) = &cfg.colors {
            let chat_bg = colors.chat_background.as_deref().and_then(parse_color);
            let input_bg = colors.input_background.as_deref().and_then(parse_color);
            let status_bg = colors.status_background.as_deref().and_then(parse_color);
            renderer.set_background_colors(chat_bg, input_bg, status_bg);
        }

        let mut input = InputEditor::new();
        input.set_monochrome(cli.no_color);
        input.set_prompt_names(context.prompts.keys().cloned().collect());
        input.set_theme_names(context.themes.keys().cloned().collect());
        if let Some(editor) = &cfg.editor {
            input.set_editor(editor.clone());
        }
        input.set_quick_model_names(crate::config::quick_models_map(cfg).into_keys().collect());
        input.load_global_history();

        let (user_tx, user_rx) = mpsc::channel::<UserEvent>(64);
        let running = Arc::new(AtomicBool::new(true));
        let event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));

        #[cfg(feature = "subagents")]
        let subagent_rx = {
            let (tx, rx) = mpsc::channel::<AgentEvent>(64);
            crate::extras::subagents::set_subagent_event_tx(tx);
            Some(rx)
        };
        #[cfg(not(feature = "subagents"))]
        let subagent_rx = None::<mpsc::Receiver<AgentEvent>>;

        Ok(InteractiveSession {
            client,
            agent,
            cli,
            cfg,
            session,
            context,
            permission,
            ask_tx,
            ask_rx,
            sandbox,

            renderer,
            input,
            user_tx,
            user_rx,
            running,
            event_handle,

            is_running: false,
            agent_rx: None,
            subagent_rx,
            agent_line_started: false,
            response_buf: String::new(),
            response_start_line: None,
            show_reasoning: true,
            reasoning_enabled: true,
            was_reasoning: false,
            todo_tools_enabled: false,
            loop_label: None,
            btw_active: false,
            btw_msg_count: 0,
            btw_input_tokens: 0,
            btw_output_tokens: 0,
            btw_cost: 0.0,
            dot_prompt_restore: None,

            #[cfg(feature = "loop")]
            loop_state: None,
            #[cfg(feature = "git-worktree")]
            wt_return_path: None,
            #[cfg(feature = "mcp")]
            mcp_manager,
        })
    }

    fn perm_mode(&self) -> Option<String> {
        self.permission.as_ref().map(|p| {
            p.lock()
                .unwrap_or_else(|e| e.into_inner())
                .mode()
                .to_string()
        })
    }

    fn refresh(&mut self) -> io::Result<()> {
        self.renderer.render_viewport()?;
        let status = StatusLine::render(
            self.session,
            self.is_running,
            0,
            self.loop_label.as_deref(),
            self.context.current_prompt_name.as_deref(),
            self.perm_mode().as_deref(),
        );
        self.renderer.draw_bottom(
            &self.input.buffer,
            self.input.cursor,
            &status,
            self.is_running,
        )?;
        if let Some(ref picker) = self.input.picker {
            picker.draw()?;
        }
        Ok(())
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        self.initialize_display()?;

        #[cfg(feature = "git-worktree")]
        self.setup_git_worktree().await;

        self.event_loop().await?;

        #[cfg(feature = "git-worktree")]
        crate::ui::worktree::handle_auto_merge(
            &mut self.renderer,
            &mut self.user_rx,
            self.cli,
            self.cfg,
        )
        .await;

        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.event_handle.take() {
            let _ = h.join();
        }

        #[cfg(feature = "mcp")]
        if let Some(mgr) = self.mcp_manager.take() {
            mgr.shutdown().await;
        }

        Ok(())
    }

    fn initialize_display(&mut self) -> anyhow::Result<()> {
        render_session(
            &mut self.renderer,
            self.session,
            self.cli,
            self.cfg,
            self.context,
        )?;

        let marker_path = crate::session::storage::data_dir().join("shown_welcome_msg");
        if !marker_path.exists() {
            self.renderer
                .write_line("──────────────────────────────────────────", Color::Cyan)?;
            self.renderer
                .write_line("  zerostack Quickstart", Color::Cyan)?;
            self.renderer
                .write_line("──────────────────────────────────────────", Color::Cyan)?;
            self.renderer.write_line("", Color::White)?;
            self.renderer.write_line("  Pickers:", C_TOOL)?;
            self.renderer.write_line(
                "    @<path>     File picker / auto-complete paths",
                Color::White,
            )?;
            self.renderer.write_line(
                "    !<command>  Run a shell command (output stored as assistant)",
                Color::White,
            )?;
            self.renderer.write_line(
                "    .<prompt>   Switch prompt or one-shot .<prompt> <message>",
                Color::White,
            )?;
            self.renderer.write_line("", Color::White)?;
            self.renderer.write_line("  Slash Commands:", C_TOOL)?;
            self.renderer
                .write_line("    /model        Switch model", Color::White)?;
            self.renderer
                .write_line("    /prompt       List / activate prompts", Color::White)?;
            self.renderer.write_line(
                "    /prompt autoconfig  Guided setup via docs",
                Color::White,
            )?;
            self.renderer
                .write_line("    /mode         Change security mode", Color::White)?;
            self.renderer
                .write_line("    /clear        Clear session", Color::White)?;
            self.renderer
                .write_line("    /undo         Undo last exchange", Color::White)?;
            self.renderer
                .write_line("    /compress     Free context window space", Color::White)?;
            self.renderer
                .write_line("    /help         Show all commands", Color::White)?;
            self.renderer.write_line("", Color::White)?;
            self.renderer.write_line(
                "  Docs: https://gi-dellav.github.io/zerostack/",
                Color::White,
            )?;
            self.renderer.write_line("", Color::White)?;
            self.renderer.write_line("  Keybindings:", C_TOOL)?;
            self.renderer
                .write_line("    Ctrl+G     Open input in $EDITOR", Color::White)?;
            self.renderer
                .write_line("    Ctrl+H     Launch lazygit", Color::White)?;
            self.renderer
                .write_line("    Ctrl+S     Save session", Color::White)?;
            self.renderer
                .write_line("    Tab        File picker / auto-complete", Color::White)?;
            self.renderer.write_line("", Color::White)?;
            self.renderer
                .write_line("──────────────────────────────────────────", Color::Cyan)?;
            self.renderer.write_line("", Color::White)?;
            if let Some(dir) = marker_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&marker_path, "");
        }

        self.refresh()?;
        Ok(())
    }

    #[cfg(feature = "git-worktree")]
    async fn setup_git_worktree(&mut self) {
        if let Some(name) = &self.cli.worktree {
            #[cfg(feature = "mcp")]
            let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
            crate::ui::worktree::setup_worktree_env(
                name,
                self.session,
                self.context,
                &self.client,
                &mut self.agent,
                &mut self.renderer,
                self.cli,
                self.cfg,
                &self.permission,
                &self.ask_tx,
                &self.sandbox,
                self.reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_ref,
            )
            .await;
        }
        if self.cli.parallel {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let name = ts.to_string();
            #[cfg(feature = "mcp")]
            let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
            crate::ui::worktree::setup_worktree_env(
                &name,
                self.session,
                self.context,
                &self.client,
                &mut self.agent,
                &mut self.renderer,
                self.cli,
                self.cfg,
                &self.permission,
                &self.ask_tx,
                &self.sandbox,
                self.reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_ref,
            )
            .await;
        }
    }

    async fn event_loop(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(ev) = self.user_rx.recv() => {
                    self.handle_user_event(ev).await?;
                }
                Some(event) = async {
                    self.agent_rx.as_mut()?.recv().await
                } => {
                    self.handle_agent_event(event).await?;
                }
                Some(event) = async {
                    self.subagent_rx.as_mut()?.recv().await
                } => {
                    self.handle_subagent_event(event).await?;
                }
                Some(ask_req) = async {
                    self.ask_rx.as_mut()?.recv().await
                } => {
                    handle_permission_request(
                        ask_req, &mut self.renderer, self.session, self.cli,
                        &mut self.user_rx, &mut self.agent_line_started, &mut self.was_reasoning,
                    ).await?;
                    self.refresh()?;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)), if self.is_running => {
                    self.refresh()?;
                }
                else => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn handle_user_event(&mut self, ev: UserEvent) -> anyhow::Result<()> {
        match ev {
            UserEvent::Resize => {
                self.renderer.resize();
                self.refresh()?;
            }
            UserEvent::ScrollUp => {
                self.renderer.scroll_line_up();
                self.refresh()?;
            }
            UserEvent::ScrollDown => {
                self.renderer.scroll_line_down();
                self.refresh()?;
            }
            UserEvent::MouseDown { row, col: _ } => {
                if row < self.renderer.visible_lines() as u16
                    && let Some(idx) = self.renderer.buffer_line_at_row(row)
                {
                    self.renderer.selection_active = true;
                    self.renderer.selection_start = Some(idx);
                    self.renderer.selection_end = Some(idx);
                    self.refresh()?;
                }
            }
            UserEvent::MouseDrag { row, col: _ } => {
                if self.renderer.selection_active
                    && let Some(idx) = self.renderer.buffer_line_at_row(row)
                {
                    self.renderer.selection_end = Some(idx);
                    self.refresh()?;
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
                    self.refresh()?;
                }
            }
            UserEvent::Paste(data) => {
                self.input.handle_paste(data);
                self.refresh()?;
            }
            UserEvent::Key(key) => {
                self.handle_key_event(key).await?;
            }
        }
        Ok(())
    }

    async fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
        let is_ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        let is_ctrl_d =
            key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL);
        if is_ctrl_c || is_ctrl_d {
            if self.is_running {
                self.is_running = false;
                self.agent_rx = None;
                #[cfg(feature = "loop")]
                if let Some(ref mut ls) = self.loop_state {
                    ls.active = false;
                    self.loop_label = None;
                }
                self.renderer.write_line("interrupted", C_ERROR)?;
                self.refresh()?;
            } else {
                // Signal exit by dropping the event handle
                if let Some(h) = self.event_handle.take() {
                    self.running.store(false, Ordering::Relaxed);
                    let _ = h.join();
                }
                return Err(anyhow::anyhow!(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "interrupted"
                )));
            }
            return Ok(());
        }

        if self.renderer.selection_active && key.code == KeyCode::Char('y') {
            if let Some(text) = self.renderer.selected_text() {
                copy_to_clipboard(&text);
                self.renderer.write_line("copied selection", Color::Green)?;
            }
            self.renderer.clear_selection();
            self.refresh()?;
            return Ok(());
        }
        if self.renderer.selection_active && key.code == KeyCode::Esc {
            self.renderer.clear_selection();
            self.refresh()?;
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
            self.refresh()?;
            return Ok(());
        }

        match key.code {
            KeyCode::PageUp => {
                self.renderer.scroll_page_up();
                self.refresh()?;
                return Ok(());
            }
            KeyCode::PageDown => {
                self.renderer.scroll_page_down();
                self.refresh()?;
                return Ok(());
            }
            KeyCode::Home => {
                self.renderer.scroll_to_top();
                self.refresh()?;
                return Ok(());
            }
            KeyCode::End => {
                self.renderer.scroll_to_bottom()?;
                self.refresh()?;
                return Ok(());
            }
            _ => {}
        }

        if self.input.picker.as_ref().is_some_and(|p| p.active())
            && self.input.handle_picker_key(key)
        {
            self.refresh()?;
            return Ok(());
        }

        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(h) = self.event_handle.take() {
                self.running.store(false, Ordering::Relaxed);
                let _ = h.join();
            }
            self.input.open_in_editor();

            let (new_tx, new_rx) = mpsc::channel(64);
            self.user_tx = new_tx;
            self.user_rx = new_rx;
            self.running = Arc::new(AtomicBool::new(true));
            self.event_handle = Some(spawn_event_thread(
                self.user_tx.clone(),
                self.running.clone(),
            ));
            self.refresh()?;
            return Ok(());
        }

        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if std::process::Command::new("lazygit")
                .arg("--version")
                .output()
                .is_err()
            {
                self.renderer.write_line(
                    "warning: lazygit not found — install it (https://github.com/jesseduffield/lazygit)",
                    C_ERROR,
                )?;
                self.refresh()?;
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

            let (new_tx, new_rx) = mpsc::channel(64);
            self.user_tx = new_tx;
            self.user_rx = new_rx;
            self.running = Arc::new(AtomicBool::new(true));
            self.event_handle = Some(spawn_event_thread(
                self.user_tx.clone(),
                self.running.clone(),
            ));
            self.refresh()?;
            return Ok(());
        }

        if let Some(mut text) = self.input.handle_key(key) {
            #[cfg(feature = "loop")]
            if self.loop_state.as_ref().is_some_and(|ls| ls.active) && !text.starts_with('/') {
                self.renderer
                    .write_line("loop active: /loop stop to cancel", C_ERROR)?;
                self.refresh()?;
                return Ok(());
            }
            if self.renderer.is_scrolling() {
                self.renderer.scroll_to_bottom()?;
            }
            self.handle_input_text(&mut text).await?;
            self.refresh()?;
        } else if self.is_running {
            self.refresh()?;
        } else {
            let status = StatusLine::render(
                self.session,
                self.is_running,
                0,
                self.loop_label.as_deref(),
                self.context.current_prompt_name.as_deref(),
                self.perm_mode().as_deref(),
            );
            self.renderer.draw_bottom(
                &self.input.buffer,
                self.input.cursor,
                &status,
                self.is_running,
            )?;
            if let Some(ref picker) = self.input.picker {
                picker.draw()?;
            }
        }
        Ok(())
    }

    async fn handle_input_text(&mut self, text: &mut CompactString) -> anyhow::Result<()> {
        let mut is_dot_cmd = false;

        if text.starts_with('.') {
            is_dot_cmd = true;
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
            } else if let Some((prompt_name, msg)) = after_dot.split_once(char::is_whitespace) {
                let prompt_name = prompt_name.trim();
                let msg = msg.trim();
                if !prompt_name.is_empty() && self.context.prompts.contains_key(prompt_name) {
                    self.dot_prompt_restore = self.context.current_prompt_name.clone();
                    if switch_prompt(self.context, &self.permission, prompt_name) {
                        *text = msg.to_string().into();
                        is_dot_cmd = false;
                    }
                } else {
                    self.renderer
                        .write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
                }
            } else {
                let prompt_name = after_dot.trim();
                if self.context.prompts.contains_key(prompt_name) {
                    if switch_prompt(self.context, &self.permission, prompt_name) {
                        report_prompt_switch(
                            &mut self.renderer,
                            self.session,
                            prompt_name,
                            self.cli,
                        );
                    }
                } else {
                    self.renderer
                        .write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
                }
            }
        }

        if !is_dot_cmd {
            if text.starts_with('/') {
                self.handle_slash(text).await?;
            } else if text.starts_with('!') {
                self.handle_shell(text).await?;
            } else {
                self.handle_normal_text(text).await?;
            }
        }

        Ok(())
    }

    async fn handle_slash(&mut self, text: &str) -> anyhow::Result<()> {
        for line in text.lines() {
            let safe_line = sanitize_output(line);
            self.renderer
                .write_line(&format!("> {}", safe_line), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;

        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;

        let result = if text.starts_with("/btw") {
            let btw_text = text.strip_prefix("/btw").map(|s| s.trim()).unwrap_or("");
            if btw_text.is_empty() {
                self.renderer.write_line("usage: /btw <message>", C_AGENT)?;
                Ok(())
            } else {
                self.btw_msg_count = self.session.messages.len();
                self.btw_input_tokens = self.session.total_input_tokens;
                self.btw_output_tokens = self.session.total_output_tokens;
                self.btw_cost = self.session.total_cost;
                ensure_agent(
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
                let history = crate::agent::runner::convert_history(self.session);
                let runner = self
                    .agent
                    .as_ref()
                    .unwrap()
                    .clone()
                    .spawn_runner(btw_text.to_string(), history);
                self.agent_rx = Some(runner.event_rx);
                self.is_running = true;
                self.btw_active = true;
                Ok(())
            }
        } else {
            handle_slash(
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
            .await
        };

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
            #[cfg(feature = "git-worktree")]
            Err(e) if e.to_string().starts_with("DEFER_WT_MERGE:") => {
                let err_msg = e.to_string();
                let parts: Vec<&str> = err_msg
                    .strip_prefix("DEFER_WT_MERGE:")
                    .unwrap_or("")
                    .splitn(5, ':')
                    .collect();
                if parts.len() == 5 {
                    let branch = parts[0];
                    let target = parts[1];
                    let main_path = parts[2].to_string();
                    let wt_path = parts[3];
                    let _repo_name = parts[4];
                    let prompt = format!(
                        "I'm in a git worktree on branch '{branch}' at '{wt_path}'. \
                         Merge it into '{target}' in the main repo at '{main_path}'.\n\n\
                         Follow these steps:\n\
                         1. cd {main_path}\n\
                         2. git fetch --all\n\
                         3. git checkout {target}\n\
                         4. git pull --no-edit\n\
                         5. git merge --no-edit {branch}\n\n\
                         After step 5, CHECK THE EXIT CODE and output.\n\
                         - If the merge Succeeded (no conflicts), continue to step 6.\n\
                         - If there is a MERGE CONFLICT:\n\
                           a. Run: git diff --name-only --diff-filter=U\n\
                           b. Tell the user WHICH FILES have conflicts. Show them the list.\n\
                           c. Ask the user what to do. Give them these options:\n\
                              - 'abort': run `git merge --abort`, do NOT push, do NOT delete anything, stop here.\n\
                              - 'resolve <file>': you help them fix the conflict in that file.\n\
                              - 'leave': leave the conflict state as-is for manual resolution.\n\
                           d. WAIT for the user's response before continuing.\n\
                           e. Follow their instruction.\n\n\
                         6. If the merge succeeded (or conflicts were resolved):\n\
                           - git push\n\
                           - git worktree remove {wt_path}\n\
                           - git branch -D {branch}\n\n\
                         7. cd {main_path} and report completion.\n\n\
                         Important: Do NOT skip any step. Always check for conflicts after merge.",
                        branch = branch,
                        wt_path = wt_path,
                        target = target,
                        main_path = main_path
                    );
                    self.session.add_message(MessageRole::User, &prompt);
                    let history = crate::agent::runner::convert_history(self.session);
                    #[cfg(feature = "mcp")]
                    let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                    ensure_agent(
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
                    let runner = self
                        .agent
                        .as_ref()
                        .unwrap()
                        .clone()
                        .spawn_runner(prompt, history);
                    self.agent_rx = Some(runner.event_rx);
                    self.is_running = true;
                    self.wt_return_path = Some(main_path);
                }
            }
            #[cfg(feature = "git-worktree")]
            Err(e) if e.to_string().starts_with("DEFER_WT_EXIT:") => {
                let err_msg = e.to_string();
                let parts: Vec<&str> = err_msg
                    .strip_prefix("DEFER_WT_EXIT:")
                    .unwrap_or("")
                    .splitn(2, ':')
                    .collect();
                if parts.len() == 2 {
                    let main_path = parts[0];
                    std::env::set_current_dir(main_path)
                        .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
                    self.session.working_dir = compact_str::CompactString::new(main_path);
                    self.context.reload();
                    apply_current_prompt_mode(self.context, &self.permission);
                    #[cfg(feature = "mcp")]
                    let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                    let model = self.client.completion_model(self.session.model.to_string());
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
                    self.renderer
                        .write_line(&format!("returned to main repo at {}", main_path), C_AGENT)?;
                }
            }
            Err(e) => {
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e: &std::io::Error| e.kind() == std::io::ErrorKind::Interrupted)
                {
                    return Err(e);
                }
                self.renderer
                    .write_line(&format!("error: {}", e), C_ERROR)?;
            }
            Ok(_) => {
                if !self.cli.no_session
                    && let Err(e) = crate::session::storage::save_session(self.session)
                {
                    self.renderer
                        .write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
                }
                #[cfg(feature = "loop")]
                if let Some(ref mut ls) = self.loop_state
                    && ls.active
                    && ls.iteration == 0
                    && !self.is_running
                {
                    ls.iteration = 1;
                    let prompt = ls.build_prompt();
                    #[cfg(feature = "mcp")]
                    let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
                    ensure_agent(
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
                    let runner = self
                        .agent
                        .as_ref()
                        .unwrap()
                        .clone()
                        .spawn_runner(prompt, Vec::new());
                    self.agent_rx = Some(runner.event_rx);
                    self.is_running = true;
                    self.loop_label = Some(ls.iteration_label());
                }
            }
        }

        if !self.cli.no_session
            && let Err(e) = crate::session::storage::save_session(self.session)
        {
            self.renderer
                .write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
        }

        Ok(())
    }

    async fn handle_shell(&mut self, text: &str) -> anyhow::Result<()> {
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

    async fn handle_normal_text(&mut self, text: &str) -> anyhow::Result<()> {
        for line in text.lines() {
            let safe_line = sanitize_output(line);
            self.renderer
                .write_line(&format!("> {}", safe_line), Color::Green)?;
        }
        self.renderer.write_line("", Color::White)?;

        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        ensure_agent(
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
        let history = crate::agent::runner::convert_history(self.session);
        let runner = self
            .agent
            .as_ref()
            .unwrap()
            .clone()
            .spawn_runner(text.to_string(), history);
        self.agent_rx = Some(runner.event_rx);
        self.is_running = true;

        self.session.add_message(MessageRole::User, text);
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

    async fn handle_agent_event(&mut self, event: AgentEvent) -> anyhow::Result<()> {
        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        handle_agent_event(
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
            &mut self.response_start_line,
            &mut self.was_reasoning,
            self.show_reasoning,
            &mut self.agent,
            &mut self.client,
            &mut self.loop_label,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            #[cfg(feature = "loop")]
            &mut self.loop_state,
            #[cfg(feature = "git-worktree")]
            &mut self.wt_return_path,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await?;

        if self.btw_active && !self.is_running {
            while self.session.messages.len() > self.btw_msg_count {
                self.session.messages.pop();
            }
            self.session.total_input_tokens = self.btw_input_tokens;
            self.session.total_output_tokens = self.btw_output_tokens;
            self.session.total_cost = self.btw_cost;
            if !self.cli.no_session {
                let _ = crate::session::storage::save_session(self.session);
            }
            self.btw_active = false;
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

        self.refresh()?;
        Ok(())
    }

    async fn handle_subagent_event(&mut self, event: AgentEvent) -> anyhow::Result<()> {
        #[cfg(feature = "mcp")]
        let mcp_ref = ensure_mcp_manager(&mut self.mcp_manager, self.cfg).await;
        handle_agent_event(
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
            &mut self.response_start_line,
            &mut self.was_reasoning,
            self.show_reasoning,
            &mut self.agent,
            &mut self.client,
            &mut self.loop_label,
            &self.permission,
            &self.ask_tx,
            &self.sandbox,
            #[cfg(feature = "loop")]
            &mut self.loop_state,
            #[cfg(feature = "git-worktree")]
            &mut self.wt_return_path,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await?;
        self.refresh()?;
        Ok(())
    }
}
