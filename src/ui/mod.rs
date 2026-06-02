mod event_handler;
pub(crate) mod events;
pub(crate) mod input;
pub(crate) mod markdown;
mod permission_handler;
pub(crate) mod pickers;
pub(crate) mod renderer;
pub(crate) mod slash;
mod status;
mod terminal;
pub(crate) mod utils;

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::config;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::{AgentEvent, UserEvent};
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::permission;
use crate::permission::ask::{AskReceiver, AskSender};
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::event_handler::{ensure_agent, handle_agent_event};
use crate::ui::events::{render_session, sanitize_output};
use crate::ui::input::InputEditor;
use crate::ui::permission_handler::handle_permission_request;
use crate::ui::renderer::{Renderer, copy_to_clipboard};
use crate::ui::slash::{handle_compress, handle_slash};
use crate::ui::status::StatusLine;
use crate::ui::terminal::TerminalGuard;

use self::utils::parse_color;

pub(crate) fn apply_current_prompt_mode(
    context: &mut ContextFiles,
    permission: &Option<PermCheck>,
) {
    let Some(content) = &context.current_prompt.clone() else {
        return;
    };
    let (mode_directive, clean_content) = permission::parse_prompt_mode(&content);
    if mode_directive.is_some() {
        context.current_prompt = Some(clean_content.to_string());
    }
    let Some(mode_str) = mode_directive else {
        return;
    };
    let Some(perm) = permission else { return };
    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
    if mode_str == "last_user_mode" {
        guard.restore_user_mode();
    } else if let Some(mode) = permission::SecurityMode::from_str(mode_str) {
        guard.set_prompt_mode(mode);
    }
}

pub(super) const C_AGENT: Color = Color::White;
pub(super) const C_ERROR: Color = Color::Red;
pub(super) const C_TOOL: Color = Color::Yellow;
pub(super) const C_PERM: Color = Color::Magenta;
pub(super) const C_BTW: Color = Color::Cyan;

#[allow(clippy::too_many_arguments)]
fn refresh_display(
    renderer: &mut Renderer,
    input: &mut InputEditor,
    session: &Session,
    is_running: bool,
    loop_label: Option<&str>,
    prompt_name: Option<&str>,
    perm_mode: Option<&str>,
    btw_cost: f64,
    btw_in: u64,
    btw_out: u64,
) -> io::Result<()> {
    renderer.render_viewport()?;
    let status = StatusLine::render(
        session,
        is_running,
        0,
        loop_label,
        prompt_name,
        perm_mode,
        btw_cost,
        btw_in,
        btw_out,
    );
    renderer.draw_bottom(&input.buffer, input.cursor, &status, is_running)?;
    if let Some(ref mut picker) = input.picker {
        picker.draw()?;
    }
    Ok(())
}

fn spawn_event_thread(
    user_tx: mpsc::Sender<UserEvent>,
    running: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            if let Ok(true) = event::poll(Duration::from_millis(50)) {
                match event::read() {
                    Ok(event::Event::Key(key)) => {
                        if key.kind == KeyEventKind::Press
                            && user_tx.blocking_send(UserEvent::Key(key)).is_err()
                        {
                            break;
                        }
                    }
                    Ok(event::Event::Mouse(m)) => match m.kind {
                        MouseEventKind::ScrollUp => {
                            if user_tx.blocking_send(UserEvent::ScrollUp).is_err() {
                                break;
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if user_tx.blocking_send(UserEvent::ScrollDown).is_err() {
                                break;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let _ = user_tx.blocking_send(UserEvent::MouseDown {
                                row: m.row,
                                col: m.column,
                            });
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            let _ = user_tx.blocking_send(UserEvent::MouseDrag {
                                row: m.row,
                                col: m.column,
                            });
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            let _ = user_tx.blocking_send(UserEvent::MouseUp {
                                row: m.row,
                                col: m.column,
                            });
                        }
                        _ => {}
                    },
                    Ok(event::Event::Resize(_cols, _rows)) => {
                        let _ = user_tx.blocking_send(UserEvent::Resize);
                    }
                    Ok(event::Event::Paste(data)) => {
                        let _ = user_tx.blocking_send(UserEvent::Paste(data));
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        }
    })
}

/// Lazily initialise the MCP client manager (connects only on first use).
#[cfg(feature = "mcp")]
async fn ensure_mcp_manager<'a>(
    mcp: &'a mut Option<McpClientManager>,
    cfg: &'a Config,
) -> Option<&'a McpClientManager> {
    if mcp.is_none()
        && let Some(servers) = &cfg.mcp_servers
    {
        *mcp = Some(McpClientManager::connect_all(servers).await);
    }
    mcp.as_ref()
}

/// What to do with a submitted line, given whether a main run is already active.
/// Pure decision so it can be unit-tested without a TUI/agent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubmitAction {
    /// Idle: start a run now.
    Run,
    /// Running + plain text: queue and replay after the current run finishes.
    Queue,
    /// Running + a command (`/`, `.`, `!`): can't queue meaningfully — tell the
    /// user to wait or Ctrl-C.
    RejectWhileRunning,
    /// Empty submit: ignore.
    Ignore,
}

/// Commands that are safe to run *even while a main run is active* because they
/// don't spawn or mutate the main run — the single "bypass" whitelist. Add
/// future parallel-safe commands here. Currently: `/queue` (queue management)
/// and `/btw` (isolated, tool-less side question on its own event stream).
pub(crate) fn allowed_while_running(text: &str) -> bool {
    let t = text.trim_start();
    t == "/queue" || t.starts_with("/queue ") || t == "/btw" || t.starts_with("/btw ")
}

pub(crate) fn classify_submission(is_running: bool, text: &str) -> SubmitAction {
    // Idle, or a whitelisted parallel-safe command → let it through to its
    // handler. Everything else, while running, is gated.
    if !is_running || allowed_while_running(text) {
        return SubmitAction::Run;
    }
    let t = text.trim_start();
    if t.is_empty() {
        SubmitAction::Ignore
    } else if t.starts_with('/') || t.starts_with('.') || t.starts_with('!') {
        SubmitAction::RejectWhileRunning
    } else {
        SubmitAction::Queue
    }
}

/// Starts a single main agent run for `text` and records its abort handle.
/// The ONLY place that sets `agent_rx`/`is_running` for user-driven runs, so the
/// "at most one main run" invariant is enforced in one spot. Callers must ensure
/// no run is already active (otherwise the previous one would be orphaned).
#[allow(clippy::too_many_arguments)]
async fn start_main_run(
    text: &str,
    agent: &mut Option<AnyAgent>,
    client: &AnyClient,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    reasoning_enabled: bool,
    agent_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
    main_abort: &mut Option<tokio::task::AbortHandle>,
    is_running: &mut bool,
    #[cfg(feature = "mcp")] mcp_manager: &mut Option<McpClientManager>,
) {
    #[cfg(feature = "mcp")]
    let mcp_ref = ensure_mcp_manager(mcp_manager, cfg).await;
    ensure_agent(
        agent,
        client,
        session,
        cli,
        cfg,
        context,
        permission,
        ask_tx,
        sandbox,
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
        .spawn_runner(text.to_string(), history);
    *agent_rx = Some(runner.event_rx);
    *main_abort = Some(runner.abort_handle);
    *is_running = true;
    session.add_message(MessageRole::User, text);
    if !cli.no_session {
        let _ = crate::session::chat_history::append_entry(
            &crate::session::chat_history::ChatHistoryEntry {
                content: text.to_string(),
                timestamp: session.updated_at.clone(),
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_interactive(
    mut client: AnyClient,
    mut agent: Option<AnyAgent>,
    cli: &Cli,
    cfg: &Config,
    session: &mut Session,
    context: &mut ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    mut ask_rx: Option<AskReceiver>,
    sandbox: Sandbox,
    auto_trigger_msg: Option<String>,
) -> anyhow::Result<()> {
    let _guard = TerminalGuard::new()?;

    #[cfg(feature = "mcp")]
    let mut mcp_manager: Option<McpClientManager> = None;

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
    input.set_quick_model_names(config::quick_models_map(cfg).into_keys().collect());
    {
        // fixed built-in providers plus any custom gateways from config
        let mut providers: Vec<String> = ["anthropic", "openai", "gemini", "openrouter", "ollama"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        providers.extend(cfg.custom_providers_map().keys().cloned());
        input.set_provider_names(providers);
    }
    input.load_global_history();
    // pre-warm the current provider's live models into the picker (best-effort)
    {
        let provider = session.provider.to_string();
        let is_custom = cfg.custom_providers_map().contains_key(&provider);
        let ids = crate::ui::slash::warm_model_cache(&provider, is_custom, &client, cli, cfg).await;
        input.set_live_model_names(ids);
    }
    let mut is_running = false;
    let mut agent_rx: Option<mpsc::Receiver<AgentEvent>> = None;
    // Abort handle for the single in-flight main run. Enforces "at most one main
    // run" and lets Ctrl-C actually cancel it (not just stop listening).
    let mut main_abort: Option<tokio::task::AbortHandle> = None;
    // Inputs submitted while a main run is active are queued here and replayed
    // when it finishes — instead of silently spawning a second run that would
    // orphan the first.
    let mut pending_inputs: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut agent_line_started = false;
    let mut response_buf = String::new();
    let mut response_start_line: Option<usize> = None;
    let mut show_reasoning = true;
    let mut reasoning_enabled = true;
    let mut was_reasoning = false;
    let mut todo_tools_enabled = false;
    #[allow(unused_mut)]
    let mut loop_label: Option<String> = None;
    #[cfg(feature = "loop")]
    let mut loop_state: Option<crate::extras::r#loop::LoopState> = None;
    #[cfg(feature = "git-worktree")]
    let mut wt_return_path: Option<String> = None;
    // `/btw` side questions run on an independent event stream — they never
    // touch `agent_rx`/`is_running`/`session`, so they can run in parallel with
    // the main agent and leave no trace in conversation history.
    let (btw_tx, mut btw_rx) = mpsc::channel::<crate::event::BtwEvent>(32);
    let mut btw_abort: Vec<(u32, tokio::task::AbortHandle)> = Vec::new();
    let mut btw_inflight: usize = 0;
    let mut btw_next_id: u32 = 0;
    let mut btw_total_cost: f64 = 0.0;
    let mut btw_total_in: u64 = 0;
    let mut btw_total_out: u64 = 0;
    // Running trace of the main agent's current (in-flight) turn, so a parallel
    // `/btw` fired mid-task can see what the agent is doing right now (the
    // session itself only records the final assistant text per turn).
    let mut turn_trace: Vec<compact_str::CompactString> = Vec::new();
    const TURN_TRACE_MAX: usize = 64;
    let mut dot_prompt_restore: Option<String> = None;

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
        btw_total_cost,
        btw_total_in,
        btw_total_out,
    )?;

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
        ensure_agent(
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
            .spawn_runner(trigger_msg.to_string(), history);
        agent_rx = Some(runner.event_rx);
        main_abort = Some(runner.abort_handle);
        is_running = true;
        session.add_message(MessageRole::User, trigger_msg);
    }

    let (mut user_tx, mut user_rx) = mpsc::channel::<UserEvent>(64);
    let mut running = Arc::new(AtomicBool::new(true));
    let mut event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));

    loop {
        tokio::select! {
            Some(ev) = user_rx.recv() => {
                match ev {
                    UserEvent::Resize => {
                        renderer.resize();
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::ScrollUp => {
                        renderer.scroll_line_up();
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::ScrollDown => {
                        renderer.scroll_line_down();
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::MouseDown { row, col: _ } => {
                        if row < renderer.visible_lines() as u16
                            && let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_active = true;
                                renderer.selection_start = Some(idx);
                                renderer.selection_end = Some(idx);
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            }
                        continue;
                    }
                    UserEvent::MouseDrag { row, col: _ } => {
                        if renderer.selection_active
                            && let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_end = Some(idx);
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            }
                        continue;
                    }
                    UserEvent::MouseUp { row, col: _ } => {
                        if renderer.selection_active {
                            if let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_end = Some(idx);
                            }
                            if let Some(text) = renderer.selected_text() {
                                copy_to_clipboard(&text);
                            }
                            renderer.clear_selection();
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        }
                        continue;
                    }
                    UserEvent::Paste(data) => {
                        input.handle_paste(data);
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::Key(key) => {
                        let is_ctrl_c = key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        let is_ctrl_d = key.code == KeyCode::Char('d')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if is_ctrl_c || is_ctrl_d {
                            if btw_inflight > 0 {
                                // Cancel in-flight side questions first, without
                                // disturbing the main agent.
                                for (_, h) in btw_abort.drain(..) {
                                    h.abort();
                                }
                                btw_inflight = 0;
                                renderer.write_line("btw cancelled", C_ERROR)?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            } else if is_running {
                                // Actually cancel the run's task (not just stop
                                // listening), so it stops executing tools. bash
                                // children are killed via kill_on_drop.
                                if let Some(h) = main_abort.take() {
                                    h.abort();
                                }
                                is_running = false;
                                agent_rx = None;
                                turn_trace.clear();
                                pending_inputs.clear();
                                #[cfg(feature = "loop")]
                                if let Some(ref mut ls) = loop_state {
                                    ls.active = false;
                                    loop_label = None;
                                }
                                if let Some(restore_name) = dot_prompt_restore.take() {
                                    context.current_prompt = context.prompts.get(&restore_name).cloned();
                                    context.current_prompt_name = if context.current_prompt.is_some() {
                                        Some(restore_name)
                                    } else {
                                        None
                                    };
                                    if let Some(perm) = &permission {
                                        let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                        guard.restore_user_mode();
                                    }
                                }
                                renderer.write_line(
                                    "interrupted (changes may be partial; review with git diff)",
                                    C_ERROR,
                                )?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            } else {
                                break;
                            }
                            continue;
                        }

                        if renderer.selection_active && key.code == KeyCode::Char('y') {
                            if let Some(text) = renderer.selected_text() {
                                copy_to_clipboard(&text);
                                renderer.write_line("copied selection", Color::Green)?;
                            }
                            renderer.clear_selection();
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }
                        if renderer.selection_active && key.code == KeyCode::Esc {
                            renderer.clear_selection();
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        let ctrl_r = key.code == KeyCode::Char('r')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if ctrl_r {
                            show_reasoning = !show_reasoning;
                            renderer.write_line(
                                &format!("reasoning visibility: {}", if show_reasoning { "on" } else { "off" }),
                                Color::White,
                            )?;
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        match key.code {
                            KeyCode::PageUp => {
                                renderer.scroll_page_up();
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            KeyCode::PageDown => {
                                renderer.scroll_page_down();
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            KeyCode::Home => {
                                renderer.scroll_to_top();
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            KeyCode::End => {
                                renderer.scroll_to_bottom()?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            _ => {}
                        }

                        if input.picker.as_ref().is_some_and(|p| p.active())
                            && input.handle_picker_key(key) {
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }

                        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            if let Some(h) = event_handle.take() {
                                running.store(false, Ordering::Relaxed);
                                let _ = h.join();
                            }
                            input.open_in_editor();
                            running = Arc::new(AtomicBool::new(true));
                            let (new_tx, new_rx) = mpsc::channel(64);
                            user_tx = new_tx;
                            user_rx = new_rx;
                            event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            if std::process::Command::new("lazygit")
                                .arg("--version")
                                .output()
                                .is_err()
                            {
                                renderer.write_line(
                                    "warning: lazygit not found — install it (https://github.com/jesseduffield/lazygit)",
                                    C_ERROR,
                                )?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            if let Some(h) = event_handle.take() {
                                running.store(false, Ordering::Relaxed);
                                let _ = h.join();
                            }
                            let _ = crossterm::terminal::disable_raw_mode();
                            let mut stdout = std::io::stdout();
                            let _ = stdout.execute(crossterm::event::DisableMouseCapture);
                            let _ = stdout.execute(crossterm::terminal::LeaveAlternateScreen);
                            let _ = stdout.flush();
                            let _ = std::process::Command::new("lazygit").status();
                            let _ = stdout.execute(crossterm::terminal::EnterAlternateScreen);
                            let _ = stdout.execute(crossterm::terminal::Clear(crossterm::terminal::ClearType::All));
                            let _ = stdout.execute(crossterm::event::EnableMouseCapture);
                            let _ = crossterm::terminal::enable_raw_mode();
                            running = Arc::new(AtomicBool::new(true));
                            let (new_tx, new_rx) = mpsc::channel(64);
                            user_tx = new_tx;
                            user_rx = new_rx;
                            event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        if let Some(mut text) = input.handle_key(key) {
                            #[cfg(feature = "loop")]
                            if loop_state.as_ref().is_some_and(|ls| ls.active) && !text.starts_with('/') {
                                renderer.write_line("loop active: /loop stop to cancel", C_ERROR)?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            if renderer.is_scrolling() {
                                renderer.scroll_to_bottom()?;
                            }
                            // A main run is active: never spawn a second one (that
                            // would silently orphan the running one — it would keep
                            // executing tools, changing files, with no history).
                            // Whitelisted parallel-safe commands (see
                            // `allowed_while_running`) classify as `Run` and fall
                            // through to their handlers below.
                            match classify_submission(is_running, &text) {
                                SubmitAction::Run => {}
                                SubmitAction::Ignore => {
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                SubmitAction::RejectWhileRunning => {
                                    renderer.write_line(
                                        "agent is running — wait for it to finish or press Ctrl-C before running a command",
                                        C_ERROR,
                                    )?;
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                SubmitAction::Queue => {
                                    pending_inputs.push_back(text.to_string());
                                    renderer.write_line(&format!("queued: {}", sanitize_output(&text)), C_TOOL)?;
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                            }
                            // Bypass-slot handlers — commands allowed while running
                            // (see `allowed_while_running`). Add future
                            // parallel-safe command handlers here.
                            {
                                let t = text.trim_start();
                                if t == "/queue" || t.starts_with("/queue ") {
                                    let arg = t.strip_prefix("/queue").unwrap_or("").trim();
                                    match arg {
                                        "clear" => {
                                            let n = pending_inputs.len();
                                            pending_inputs.clear();
                                            renderer.write_line(&format!("queue cleared ({} removed)", n), C_TOOL)?;
                                        }
                                        "pop" => match pending_inputs.pop_back() {
                                            Some(x) => renderer.write_line(&format!("unqueued: {}", sanitize_output(&x)), C_TOOL)?,
                                            None => renderer.write_line("queue is empty", C_TOOL)?,
                                        },
                                        "" | "ls" | "list" => {
                                            if pending_inputs.is_empty() {
                                                renderer.write_line("queue is empty", C_TOOL)?;
                                            } else {
                                                renderer.write_line(&format!("queued ({}):", pending_inputs.len()), C_TOOL)?;
                                                for (i, q) in pending_inputs.iter().enumerate() {
                                                    renderer.write_line(&format!("  {}. {}", i + 1, sanitize_output(q)), C_TOOL)?;
                                                }
                                            }
                                        }
                                        _ => renderer.write_line("usage: /queue [ls|clear|pop]", C_ERROR)?,
                                    }
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                            }
                            // `/btw`: fork an isolated, tool-less, single-turn side
                            // question. The snapshot is taken by value here and never
                            // written to the session, so there is nothing to roll
                            // back. It runs on its own `btw_tx`/`btw_rx` stream and
                            // never touches `agent_rx`/`is_running`/`session`, so it
                            // works in parallel whether or not the main run is busy.
                            {
                                let t = text.trim_start();
                                if t == "/btw" || t.starts_with("/btw ") {
                                    for line in text.lines() {
                                        renderer.write_line(&format!("> {}", sanitize_output(line)), Color::Green)?;
                                    }
                                    renderer.write_line("", Color::White)?;
                                    let btw_text = t.strip_prefix("/btw").map(|s| s.trim()).unwrap_or("");
                                    if btw_text.is_empty() {
                                        renderer.write_line("usage: /btw <message>", C_AGENT)?;
                                    } else {
                                        let id = btw_next_id;
                                        btw_next_id = btw_next_id.wrapping_add(1);
                                        let snapshot = crate::agent::runner::build_btw_snapshot(
                                            session, &turn_trace, is_running,
                                        );
                                        let model = client.completion_model(session.model.to_string());
                                        let btw_agent = crate::provider::build_btw_agent(
                                            model, cli, cfg, context, &permission, &ask_tx, reasoning_enabled,
                                        );
                                        let runner = btw_agent.spawn_btw(
                                            btw_text.to_string(), snapshot, btw_tx.clone(), id,
                                        );
                                        btw_abort.push((id, runner.abort_handle));
                                        btw_inflight += 1;
                                        renderer.write_line(&format!("[btw #{}] thinking...", id), C_BTW)?;
                                    }
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                            }
                            let mut is_dot_cmd = false;
                            if text.starts_with('.') {
                                is_dot_cmd = true;
                                let after_dot = text[1..].trim_start();

                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                }
                                renderer.write_line("", Color::White)?;

                                if after_dot.is_empty() {
                                    input.buffer = ".".into();
                                    input.cursor = 1;
                                    input.start_dot_picker();
                                } else if let Some((prompt_name, msg)) = after_dot.split_once(char::is_whitespace) {
                                    let prompt_name = prompt_name.trim();
                                    let msg = msg.trim();
                                    if !prompt_name.is_empty() && context.prompts.contains_key(prompt_name) {
                                        dot_prompt_restore = context.current_prompt_name.clone();
                                        if let Some(content) = context.prompts.get(prompt_name).cloned() {
                                            let (mode_directive_str, clean_content) = crate::permission::parse_prompt_mode(&content);
                                            let mode_directive = mode_directive_str.map(|s| s.to_string());
                                            context.current_prompt = Some(if mode_directive.is_some() {
                                                clean_content.to_string()
                                            } else {
                                                content
                                            });
                                            context.current_prompt_name = Some(prompt_name.to_string());
                                            if let Some(ref mode_str) = mode_directive {
                                                if let Some(perm) = &permission {
                                                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                                    if mode_str == "last_user_mode" {
                                                        guard.restore_user_mode();
                                                    } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str) {
                                                        guard.set_prompt_mode(mode);
                                                    }
                                                }
                                            }
                                        }
                                        text = msg.to_string().into();
                                        is_dot_cmd = false;
                                    } else {
                                        renderer.write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
                                    }
                                } else {
                                    let prompt_name = after_dot.trim();
                                    if context.prompts.contains_key(prompt_name) {
                                        if let Some(content) = context.prompts.get(prompt_name).cloned() {
                                            let (mode_directive_str, clean_content) = crate::permission::parse_prompt_mode(&content);
                                            let mode_directive = mode_directive_str.map(|s| s.to_string());
                                            context.current_prompt = Some(if mode_directive.is_some() {
                                                clean_content.to_string()
                                            } else {
                                                content
                                            });
                                            context.current_prompt_name = Some(prompt_name.to_string());
                                            if let Some(ref mode_str) = mode_directive {
                                                if let Some(perm) = &permission {
                                                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                                    if mode_str == "last_user_mode" {
                                                        guard.restore_user_mode();
                                                    } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str) {
                                                        guard.set_prompt_mode(mode);
                                                    }
                                                }
                                            }
                                        }
                                        renderer.write_line(&format!("switched to prompt '{}'", prompt_name), C_AGENT)?;
                                        if !cli.no_session
                                            && let Err(e) = crate::session::storage::save_session(session)
                                        {
                                            renderer.write_line(
                                                &format!("warning: failed to save session: {}", e),
                                                C_ERROR,
                                            )?;
                                        }
                                    } else {
                                        renderer.write_line(&format!("error: unknown prompt '{}'", prompt_name), C_ERROR)?;
                                    }
                                }
                            }
                            if !is_dot_cmd {
                            if text.starts_with('/') {
                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                }
                                renderer.write_line("", Color::White)?;
                                #[cfg(feature = "mcp")]
                                let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                // `/btw` is handled earlier in the bypass-slot (it
                                // runs in parallel and never goes through the main
                                // run), so it never reaches here.
                                let result = handle_slash(&text, &mut agent, &mut client, &mut renderer, session, cli, cfg, context, &mut show_reasoning, &mut reasoning_enabled, &mut is_running, &mut input, &permission, &ask_tx, &mut todo_tools_enabled, &sandbox, #[cfg(feature = "loop")] &mut loop_state, #[cfg(feature = "mcp")] mcp_ref).await;
                                // provider may have changed via /provider or /models — re-warm the picker's live list
                                {
                                    let provider = session.provider.to_string();
                                    let is_custom = cfg.custom_providers_map().contains_key(&provider);
                                    let ids = crate::ui::slash::warm_model_cache(&provider, is_custom, &client, cli, cfg).await;
                                    input.set_live_model_names(ids);
                                }
                                match result {
                                Err(e) if e.to_string().starts_with("DEFER_COMPRESS:") => {
                                    let err_msg = e.to_string();
                                    let instructions = err_msg.strip_prefix("DEFER_COMPRESS:").and_then(|s| {
                                        let s = s.trim();
                                        if s.is_empty() || s == "(none)" { None } else { Some(s.to_string()) }
                                    });
                                        #[cfg(feature = "mcp")]
                                        let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                        let compress_result = handle_compress(
                                            instructions.as_deref(),
                                            &mut agent, &mut client, &mut renderer, session, cli, cfg, context,
                                            reasoning_enabled,
                                            &permission, &ask_tx, &sandbox,
                                            #[cfg(feature = "mcp")] mcp_ref,
                                        ).await;
                                        if let Err(e) = compress_result {
                                            renderer.write_line(&format!("compress error: {}", e), C_ERROR)?;
                                        }
                                        let _ = crate::session::storage::save_session(session);
                                    }
                                    #[cfg(feature = "git-worktree")]
                                    Err(e) if e.to_string().starts_with("DEFER_WT_MERGE\u{1F}") => {
                                        let err_msg = e.to_string();
                                        let parts: Vec<&str> = err_msg.strip_prefix("DEFER_WT_MERGE\u{1F}").unwrap_or("").splitn(5, '\u{1F}').collect();
                                        if parts.len() == 5 {
                                            let branch = parts[0];
                                            let target = parts[1];
                                            let main_path = parts[2].to_string();
                                            let wt_path = parts[3];
                                            let _repo_name = parts[4];
                                            #[cfg(feature = "git-worktree")]
                                            let force_flag = cli.resolve_wt_force(cfg);
                                            #[cfg(not(feature = "git-worktree"))]
                                            let force_flag = false;
                                            let wt_remove_flag = if force_flag { "--force" } else { "" };
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
                                                   - git worktree remove {wt_remove_flag} {wt_path}\n\
                                                   - git branch -D {branch}\n\n\
                                                 7. cd {main_path} and report completion.\n\n\
                                                 Important: Do NOT skip any step. Always check for conflicts after merge.",
                                                branch = branch, wt_path = wt_path, target = target, main_path = main_path,
                                                wt_remove_flag = wt_remove_flag
                                            );
                                            session.add_message(MessageRole::User, &prompt);
                                            let history = crate::agent::runner::convert_history(session);
                                            #[cfg(feature = "mcp")]
                                            let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                            ensure_agent(
                                                &mut agent, &client, session, cli, cfg, context,
                                                &permission, &ask_tx, &sandbox, reasoning_enabled,
                                                #[cfg(feature = "mcp")] mcp_ref,
                                            ).await;
                                            let runner = agent.as_ref().unwrap().clone().spawn_runner(prompt, history);
                                            agent_rx = Some(runner.event_rx);
                                            main_abort = Some(runner.abort_handle);
                                            is_running = true;
                                            wt_return_path = Some(main_path);
                                        }
                                    }
                                    #[cfg(feature = "git-worktree")]
                                    Err(e) if e.to_string().starts_with("DEFER_WT_EXIT\u{1F}") => {
                                        let err_msg = e.to_string();
                                        let parts: Vec<&str> = err_msg.strip_prefix("DEFER_WT_EXIT\u{1F}").unwrap_or("").splitn(2, '\u{1F}').collect();
                                        if parts.len() == 2 {
                                            let main_path = parts[0];
                                            std::env::set_current_dir(main_path)
                                                .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
                                            session.working_dir = compact_str::CompactString::new(main_path);
                                            context.reload();
                                            apply_current_prompt_mode(context, &permission);
                                            #[cfg(feature = "mcp")]
                                            let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                            let model = client.completion_model(session.model.to_string());
                                            agent = Some(crate::provider::build_agent(
                                                model,
                                                cli,
                                                cfg,
                                                context,
                                                permission.clone(),
                                                ask_tx.clone(),
                                                sandbox.clone(),
                                                reasoning_enabled,
                                                #[cfg(feature = "mcp")] mcp_ref,
                                            ).await);
                                            render_session(&mut renderer, session, cli, cfg, context)?;
                                            renderer.write_line(
                                                &format!("returned to main repo at {}", main_path),
                                                C_AGENT,
                                            )?;
                                        }
                                    }
                                    Err(e) if e.to_string().starts_with("DEFER_INIT:") => {
                                        let prompt = e.to_string().strip_prefix("DEFER_INIT:").unwrap_or("").to_string();
                                        #[cfg(feature = "mcp")]
                                        let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                        ensure_agent(
                                            &mut agent, &client, session, cli, cfg, context,
                                            &permission, &ask_tx, &sandbox, reasoning_enabled,
                                            #[cfg(feature = "mcp")] mcp_ref,
                                        ).await;
                                        let history = crate::agent::runner::convert_history(session);
                                        let runner = agent.as_ref().unwrap().clone().spawn_runner(prompt, history);
                                        agent_rx = Some(runner.event_rx);
                                        main_abort = Some(runner.abort_handle);
                                        is_running = true;
                                    }
                                    Err(e) if e.to_string().starts_with("DEFER_EDITOR:") => {
                                        let path = e.to_string().strip_prefix("DEFER_EDITOR:").unwrap_or("").to_string();
                                        let editor = cfg.editor.clone()
                                            .or_else(|| std::env::var("EDITOR").ok())
                                            .unwrap_or_else(|| "editor".to_string());
                                        let _ = crossterm::terminal::disable_raw_mode();
                                        let mut stdout = std::io::stdout();
                                        let _ = crossterm::ExecutableCommand::execute(&mut stdout, crossterm::event::DisableMouseCapture);
                                        let _ = crossterm::ExecutableCommand::execute(&mut stdout, crossterm::terminal::LeaveAlternateScreen);
                                        let _ = stdout.flush();
                                        let _ = std::process::Command::new("sh")
                                            .arg("-c")
                                            .arg(format!("{} \"$1\"", editor))
                                            .arg("sh")
                                            .arg(&path)
                                            .status();
                                        let _ = crossterm::ExecutableCommand::execute(&mut stdout, crossterm::terminal::EnterAlternateScreen);
                                        let _ = crossterm::ExecutableCommand::execute(&mut stdout, crossterm::terminal::Clear(crossterm::terminal::ClearType::All));
                                        let _ = crossterm::ExecutableCommand::execute(&mut stdout, crossterm::event::EnableMouseCapture);
                                        let _ = crossterm::terminal::enable_raw_mode();
                                        render_session(&mut renderer, session, cli, cfg, context)?;
                                        renderer.write_line(&format!("returned from editing {}", path), C_AGENT)?;
                                    }
                                    Err(e) => {
                                        if e.downcast_ref::<std::io::Error>().is_some_and(|e: &std::io::Error| e.kind() == std::io::ErrorKind::Interrupted) {
                                            break;
                                        }
                                        renderer.write_line(&format!("error: {}", e), C_ERROR)?;
                                    }
                                    Ok(_) => {
                                        if !cli.no_session
                                            && let Err(e) = crate::session::storage::save_session(session)
                                        {
                                            renderer.write_line(
                                                &format!("warning: failed to save session: {}", e),
                                                C_ERROR,
                                            )?;
                                        }
                                        #[cfg(feature = "loop")]
                                        if let Some(ref mut ls) = loop_state
                                            && ls.active && ls.iteration == 0 && !is_running
                                        {
                                            ls.iteration = 1;
                                            let prompt = ls.build_prompt();
                                            #[cfg(feature = "mcp")]
                                            let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                            ensure_agent(
                                                &mut agent, &client, session, cli, cfg, context,
                                                &permission, &ask_tx, &sandbox, reasoning_enabled,
                                                #[cfg(feature = "mcp")] mcp_ref,
                                            ).await;
                                            let runner = agent.as_ref().unwrap().clone().spawn_runner(prompt, Vec::new());
                                            agent_rx = Some(runner.event_rx);
                                            main_abort = Some(runner.abort_handle);
                                            is_running = true;
                                            loop_label = Some(ls.iteration_label());
                                        }
                                    }
                                }
                                if !cli.no_session
                                    && let Err(e) = crate::session::storage::save_session(session)
                                {
                                    renderer.write_line(
                                        &format!("warning: failed to save session: {}", e),
                                        C_ERROR,
                                    )?;
                                }
                            } else if text.starts_with('!') {
                                let cmd = text.strip_prefix('!').map(|s| s.trim()).unwrap_or("");
                                if !cmd.is_empty() {
                                    for line in text.lines() {
                                        let safe_line = sanitize_output(line);
                                        renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                    }
                                    renderer.write_line("", Color::White)?;

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
                                        renderer.write_line(
                                            &safe_line,
                                            if output.status.success() { C_AGENT } else { C_ERROR },
                                        )?;
                                    }
                                    renderer.write_line("", Color::White)?;

                                    session.add_message(MessageRole::User, &text);
                                    session.add_message(MessageRole::Assistant, &result);
                                    if !cli.no_session {
                                        let _ = crate::session::chat_history::append_entry(
                                            &crate::session::chat_history::ChatHistoryEntry {
                                                content: text.to_string(),
                                                timestamp: session.updated_at.clone(),
                                            },
                                        );
                                    }
                                } else {
                                    renderer.write_line("error: empty command after '!'", C_ERROR)?;
                                }
                            } else {
                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                }
                                renderer.write_line("", Color::White)?;

                                // Guaranteed not running here (the is_running gate
                                // above returns early), so this never orphans a run.
                                start_main_run(
                                    &text, &mut agent, &client, session, cli, cfg, context,
                                    &permission, &ask_tx, &sandbox, reasoning_enabled,
                                    &mut agent_rx, &mut main_abort, &mut is_running,
                                    #[cfg(feature = "mcp")] &mut mcp_manager,
                                ).await;
                            }
                            }
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        } else if is_running {
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        } else {
                            let status = StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out);
                            renderer.draw_bottom(&input.buffer, input.cursor, &status, is_running)?;
                            if let Some(ref mut picker) = input.picker {
                                picker.draw()?;
                            }
                        }
                    }
                }
            }
            Some(event) = async {
                agent_rx.as_mut()?.recv().await
            } => {
                // Accumulate a live trace of the current turn so a parallel
                // `/btw` can report what the agent is doing right now. The
                // session itself only stores the final assistant text per turn.
                match &event {
                    AgentEvent::ToolCall { name, args } => {
                        if turn_trace.len() < TURN_TRACE_MAX {
                            turn_trace.push(compact_str::CompactString::from(format!(
                                "→ {}",
                                crate::ui::utils::format_tool_call_summary(name, args)
                            )));
                        }
                    }
                    AgentEvent::ToolResult { output, .. } => {
                        if turn_trace.len() < TURN_TRACE_MAX {
                            turn_trace.push(compact_str::CompactString::from(format!(
                                "← {}",
                                crate::extras::truncate::truncate_cjk(output, 500, "…")
                            )));
                        }
                    }
                    AgentEvent::Done { .. } | AgentEvent::Error(_) => turn_trace.clear(),
                    _ => {}
                }
                #[cfg(feature = "mcp")]
                let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                handle_agent_event(
                    event, &mut renderer, session, cfg, cli, context,
                    &mut is_running, &mut agent_rx, &mut agent_line_started,
                    &mut response_buf, &mut response_start_line, &mut was_reasoning,
                    show_reasoning,
                    &mut agent, &mut client, &mut loop_label,
                    &permission, &ask_tx, &sandbox,
                    #[cfg(feature = "loop")] &mut loop_state,
                    #[cfg(feature = "git-worktree")] &mut wt_return_path,
                    #[cfg(feature = "mcp")] mcp_ref,
                ).await?;
                if !is_running
                    && let Some(restore_name) = dot_prompt_restore.take()
                {
                    context.current_prompt = context.prompts.get(&restore_name).cloned();
                    context.current_prompt_name = if context.current_prompt.is_some() {
                        Some(restore_name)
                    } else {
                        None
                    };
                    if let Some(perm) = &permission {
                        let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                        guard.restore_user_mode();
                    }
                }
                // Run finished: drop its (now-dead) abort handle and, if the user
                // queued input while it ran, replay the next one as a new run.
                if !is_running {
                    main_abort = None;
                    if let Some(next) = pending_inputs.pop_front() {
                        for line in next.lines() {
                            renderer.write_line(&format!("> {}", sanitize_output(line)), Color::Green)?;
                        }
                        renderer.write_line("", Color::White)?;
                        start_main_run(
                            &next, &mut agent, &client, session, cli, cfg, context,
                            &permission, &ask_tx, &sandbox, reasoning_enabled,
                            &mut agent_rx, &mut main_abort, &mut is_running,
                            #[cfg(feature = "mcp")] &mut mcp_manager,
                        ).await;
                    }
                }
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            Some(ask_req) = async {
                ask_rx.as_mut()?.recv().await
            } => {
                handle_permission_request(
                    ask_req, &mut renderer, session, cli,
                    &mut user_rx, &mut agent_line_started, &mut was_reasoning,
                ).await?;
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            Some(bev) = btw_rx.recv() => {
                // Parallel side-question result. Rendered as a single block; it is
                // NEVER written to the session (cost is tracked separately).
                match bev {
                    crate::event::BtwEvent::Done { id, response, input_tokens, output_tokens } => {
                        btw_total_cost += crate::pricing::estimate_cost(
                            input_tokens, output_tokens,
                            session.input_token_cost, session.output_token_cost,
                        );
                        btw_total_in = btw_total_in.saturating_add(input_tokens);
                        btw_total_out = btw_total_out.saturating_add(output_tokens);
                        btw_abort.retain(|(i, _)| *i != id);
                        btw_inflight = btw_inflight.saturating_sub(1);
                        renderer.write_line(&format!("[btw #{}] answer:", id), C_BTW)?;
                        for line in response.lines() {
                            renderer.write_line(&sanitize_output(line), C_AGENT)?;
                        }
                        renderer.write_line("", Color::White)?;
                    }
                    crate::event::BtwEvent::Error { id, message } => {
                        btw_abort.retain(|(i, _)| *i != id);
                        btw_inflight = btw_inflight.saturating_sub(1);
                        renderer.write_line(&format!("[btw #{}] error: {}", id, sanitize_output(&message)), C_ERROR)?;
                    }
                }
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)), if is_running => {
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            else => {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        }
    }

    #[cfg(feature = "git-worktree")]
    if cli.resolve_wt_auto_merge(cfg)
        && let Some(info) = crate::extras::git_worktree::detect()
    {
        let target = crate::extras::git_worktree::default_branch(&info.main_repo_path)
            .unwrap_or_else(|| "main".to_string());

        let _ = renderer.write_line(
            &format!(
                "auto-merging worktree '{}' into '{}'...",
                info.branch, target
            ),
            C_AGENT,
        );
        let (state, outcome) = crate::extras::git_worktree::try_merge(&info, &target);
        match outcome {
            crate::extras::git_worktree::MergeOutcome::Success => {
                let merge_result = if cli.resolve_wt_force(cfg) {
                    crate::extras::git_worktree::complete_merge_force(&state)
                } else {
                    crate::extras::git_worktree::complete_merge(&state)
                };
                match merge_result {
                    Ok(()) => {
                        let _ = renderer.write_line(
                            &format!("merged '{}' into '{}' and cleaned up", info.branch, target),
                            C_AGENT,
                        );
                    }
                    Err(e) => {
                        let _ = renderer.write_line(
                            &format!("merge succeeded but cleanup failed: {}", e),
                            C_ERROR,
                        );
                    }
                }
            }
            crate::extras::git_worktree::MergeOutcome::Conflicts(files) => {
                let _ = renderer.write_line(
                    &format!("merge conflict in {} file(s):", files.len()),
                    C_ERROR,
                );
                for f in &files {
                    let _ = renderer.write_line(&format!("  {}", f), C_ERROR);
                }
                let _ = renderer
                    .write_line("Keep conflict state for manual resolution? [y/N] ", C_PERM);

                let abort = loop {
                    tokio::select! {
                        Some(ev) = user_rx.recv() => {
                            if let crate::event::UserEvent::Key(key) = ev {
                                match key.code {
                                    KeyCode::Char('y') | KeyCode::Char('Y') => break false,
                                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Enter => break true,
                                    _ => {}
                                }
                            }
                        }
                    }
                };

                if abort {
                    let _ = crate::extras::git_worktree::cancel_merge(&state);
                    let _ = renderer.write_line("merge aborted, restored original state", C_AGENT);
                } else {
                    let _ = renderer.write_line(
                        &format!(
                            "conflict state left in {} for manual resolution",
                            info.main_repo_path.display()
                        ),
                        C_AGENT,
                    );
                }
            }
            crate::extras::git_worktree::MergeOutcome::Error(e) => {
                let _ = renderer.write_line(&format!("merge failed: {}", e), C_ERROR);
            }
        }
    }

    running.store(false, Ordering::Relaxed);
    if let Some(h) = event_handle.take() {
        let _ = h.join();
    }

    #[cfg(feature = "mcp")]
    if let Some(mgr) = mcp_manager {
        mgr.shutdown().await;
    }

    Ok(())
}
