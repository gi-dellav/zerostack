mod app;
mod event_handler;
pub(crate) mod events;
pub(crate) mod input;
pub(crate) mod markdown;
mod permission_handler;
pub(crate) mod pickers;
pub(crate) mod renderer;
pub(crate) mod slash;
pub(crate) mod statusline;
mod terminal;
pub(crate) mod utils;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event;
use crossterm::event::{KeyEventKind, MouseButton, MouseEventKind};
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::{AgentEvent, UserEvent};
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::extras::status_signals::StatusSignals;
use crate::permission;
use crate::permission::ask::{AskReceiver, AskSender};
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::event_handler::ensure_agent;
#[cfg(feature = "advisor")]
use crate::ui::events::sanitize_output;
use crate::ui::input::InputEditor;
use crate::ui::renderer::Renderer;
use crate::ui::slash::handle_compress;

pub(crate) fn apply_current_prompt_mode(
    context: &mut ContextFiles,
    permission: &Option<PermCheck>,
) {
    let Some(content) = &context.current_prompt.clone() else {
        return;
    };
    let (mode_directive, clean_content) = permission::parse_prompt_mode(content);
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
#[cfg(feature = "advisor")]
pub(super) const C_HANDOFF: Color = Color::Green;

#[allow(clippy::too_many_arguments)]
pub(crate) fn refresh_display(
    renderer: &mut Renderer,
    input: &mut InputEditor,
    session: &Session,
    is_running: bool,
    loop_label: Option<&str>,
    prompt_name: Option<&str>,
    perm_mode: Option<&str>,
    chain_label: Option<&str>,
    btw_cost: f64,
    btw_in: u64,
    btw_out: u64,
) -> io::Result<()> {
    // Reconcile the input height first so the chat viewport is drawn against
    // the size the input is about to occupy (avoids a stale separator when the
    // input shrinks, or chat text hidden under it when the input grows).
    renderer.sync_input_height(&input.buffer)?;
    renderer.render_viewport()?;
    let statusline_ctx = crate::ui::statusline::StatusContext {
        loop_label,
        prompt_name,
        perm_mode,
        chain_label,
        btw_cost,
        btw_in,
        btw_out,
    };
    let statusline = crate::ui::statusline::build(session, &statusline_ctx);
    renderer.draw_bottom(&input.buffer, input.cursor, &statusline, is_running)?;
    if let Some(ref mut picker) = input.picker {
        picker.draw()?;
    }
    Ok(())
}

pub(crate) fn spawn_event_thread(
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
pub(crate) async fn ensure_mcp_manager<'a>(
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

/// Build the rewind picker's list of `(message_index, preview)` for every user
/// turn in the conversation, oldest first. Only user turns are offered: a rewind
/// lands just before a message you sent, dropping everything after it.
pub(crate) fn rewind_targets(session: &Session) -> Vec<(usize, String)> {
    session
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == MessageRole::User)
        .map(|(idx, m)| {
            let preview: String = m.content.chars().take(80).collect();
            (idx, preview.replace('\n', " "))
        })
        .collect()
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

#[cfg(feature = "git-worktree")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_merge_agent(
    branch: &str,
    target: &str,
    main_path: &str,
    wt_path: &str,
    force_flag: bool,
    session: &mut Session,
    agent: &mut Option<AnyAgent>,
    client: &AnyClient,
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
    status_signals: &Option<StatusSignals>,
    wt_return_path: &mut Option<(String, String, String, bool)>,
    #[cfg(feature = "mcp")] mcp_manager: &mut Option<McpClientManager>,
) {
    let wt_remove_flag = if force_flag { "--force" } else { "" };
    let branch_delete_flag = if force_flag { "-D" } else { "-d" };
    let prompt = format!(
        "I'm in a git worktree on branch '{branch}' at '{wt_path}'. \
         Merge it into '{target}' in the main repo at '{main_path}'.\n\n\
         Follow these steps:\n\
         1. cd {main_path}\n\
         2. git fetch --all\n\
         3. git checkout {target}\n\
         4. git pull --no-edit\n\
         5. git merge --squash {branch}\n\
         6. git commit --no-edit\n\n\
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
         7. If the merge succeeded (or conflicts were resolved):\n\
           - git worktree remove {wt_remove_flag} {wt_path}\n\
           - git branch {branch_delete_flag} {branch}\n\n\
         8. cd {main_path} and report completion.\n\n\
         Important: Do NOT skip any step. Always check for conflicts after merge.",
        branch = branch,
        wt_path = wt_path,
        target = target,
        main_path = main_path,
        wt_remove_flag = wt_remove_flag,
        branch_delete_flag = branch_delete_flag
    );
    session.add_message(MessageRole::User, &prompt);
    let history = crate::agent::runner::convert_history(session);
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
    let runner = agent
        .as_ref()
        .unwrap()
        .clone()
        .spawn_runner(
            prompt,
            history,
            cfg.retry.clone(),
            #[cfg(feature = "hooks")]
            None,
        )
        .await;
    *agent_rx = Some(runner.event_rx);
    *main_abort = Some(runner.abort_handle);
    *is_running = true;
    if let Some(ss) = status_signals.as_ref() {
        ss.send_start();
    }
    *wt_return_path = Some((
        main_path.to_string(),
        wt_path.to_string(),
        branch.to_string(),
        force_flag,
    ));
}
/// Result of a background agent prebuild.
#[cfg(feature = "mcp")]
pub(crate) type PrebuildPayload = (AnyAgent, Option<McpClientManager>);
#[cfg(not(feature = "mcp"))]
pub(crate) type PrebuildPayload = AnyAgent;

/// If the background prebuild hasn't delivered yet, block until it does.
#[cfg(feature = "mcp")]
pub(crate) async fn resolve_prebuild<'a>(
    agent: &'a mut Option<AnyAgent>,
    mcp_manager: &'a mut Option<McpClientManager>,
    prebuild_rx: &'a mut Option<mpsc::Receiver<PrebuildPayload>>,
) {
    if agent.is_some() {
        return;
    }
    if let Some(rx) = prebuild_rx.as_mut() {
        if let Some((a, mcp)) = rx.recv().await {
            *agent = Some(a);
            *mcp_manager = mcp;
        }
        *prebuild_rx = None;
    }
}

#[cfg(not(feature = "mcp"))]
pub(crate) fn resolve_prebuild<'a>(
    agent: &'a mut Option<AnyAgent>,
    prebuild_rx: &'a mut Option<mpsc::Receiver<PrebuildPayload>>,
) -> impl std::future::Future<Output = ()> + 'a {
    async move {
        if agent.is_some() {
            return;
        }
        if let Some(rx) = prebuild_rx.as_mut() {
            if let Some(a) = rx.recv().await {
                *agent = Some(a);
            }
            *prebuild_rx = None;
        }
    }
}

/// Starts a single main agent run for `text` and records its abort handle.
/// The ONLY place that sets `agent_rx`/`is_running` for user-driven runs, so the
/// "at most one main run" invariant is enforced in one spot. Callers must ensure
/// no run is already active (otherwise the previous one would be orphaned).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_main_run(
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
    status_signals: &Option<StatusSignals>,
    #[cfg(feature = "mcp")] mcp_manager: &mut Option<McpClientManager>,
    prebuild_rx: &mut Option<mpsc::Receiver<PrebuildPayload>>,
    pending_send: &mut Option<String>,
) {
    // Wait for the background prebuild if it hasn't completed yet.
    #[cfg(feature = "mcp")]
    resolve_prebuild(agent, mcp_manager, prebuild_rx).await;
    #[cfg(not(feature = "mcp"))]
    resolve_prebuild(agent, prebuild_rx).await;

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
    #[cfg(feature = "multimodal")]
    let history = {
        let media = session.drain_media();
        if media.is_empty() {
            history
        } else {
            let mut h = history;
            h.extend(crate::agent::runner::media_to_messages(&media));
            h
        }
    };
    let runner = agent
        .as_ref()
        .unwrap()
        .clone()
        .spawn_runner(
            text.to_string(),
            history,
            cfg.retry.clone(),
            #[cfg(feature = "hooks")]
            None,
        )
        .await;
    *agent_rx = Some(runner.event_rx);
    *main_abort = Some(runner.abort_handle);
    *is_running = true;
    if let Some(ss) = status_signals.as_ref() {
        ss.send_start();
    }
    session.add_message(MessageRole::User, text);
    // Mark this message as the rollback target if the turn fails (see the
    // failed-send handling in the main event loop).
    *pending_send = Some(text.to_string());
    #[cfg(feature = "advisor")]
    crate::extras::advisor::set_session_messages(session.messages.clone());
    if !cli.no_session {
        let _ = crate::session::chat_history::append_entry(
            &crate::session::chat_history::ChatHistoryEntry {
                content: text.to_string(),
                timestamp: session.updated_at.clone(),
            },
        );
    }
}

/// Continuation prompt injected after a mid-turn compaction. Hardcoded as a
/// `const` rather than a `prompts/*.md` file: every `.md` under `prompts/` is
/// loaded as a selectable mode, so a file here would pollute the prompt picker.
/// Acknowledging the compaction is deliberate — it frames the summary as "what
/// I already did," not as new user instructions. The narrow-tool-calls line is
/// always present because any mid-turn fire means the configured ceiling was
/// hit, so the urgency always applies.
const MID_TURN_CONTINUE_PROMPT: &str = "[Context was compacted to save space; \
the full prior history is in the system summary above.]\n\nContinue with the \
user's original task. Do not redo work already completed per the summary; focus \
on what remains. Context was tight, so prefer narrower follow-up tool calls over \
wide ones until pressure subsides.";

/// Mid-turn auto-compaction (PR H). Invoked when real provider prompt pressure
/// (`CompletionCall` usage / context window) crosses
/// `mid_turn_compact_threshold`, and only when `compact_enabled` is true.
///
/// The in-flight run is aborted at the `CompletionCall` boundary — the model's
/// just-returned tool calls have not executed yet, so nothing is left half
/// applied. This turn's progress is recorded as a recap message (tool traffic
/// lives only in the now-aborted runner and never reaches the session, so
/// without this the agent would redo the turn), the session is compacted, and
/// the agent is respawned on the compacted history with a continuation prompt.
/// The dominant pressure relief is dropping the aborted run's in-flight tool
/// context, which the respawn achieves even when the session itself is under the
/// between-turn limit and `handle_compress` is a no-op.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn mid_turn_compact_and_respawn(
    pressure: f64,
    renderer: &mut Renderer,
    agent: &mut Option<AnyAgent>,
    client: &mut AnyClient,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &mut ContextFiles,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    reasoning_enabled: bool,
    agent_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
    main_abort: &mut Option<tokio::task::AbortHandle>,
    is_running: &mut bool,
    status_signals: &Option<StatusSignals>,
    turn_trace: &mut Vec<compact_str::CompactString>,
    response_buf: &mut String,
    response_start_line: &mut Option<usize>,
    agent_line_started: &mut bool,
    was_reasoning: &mut bool,
    #[cfg(feature = "mcp")] mcp_manager: &mut Option<McpClientManager>,
) -> anyhow::Result<()> {
    // 1. Stop the in-flight run. bash children die via kill_on_drop.
    if let Some(h) = main_abort.take() {
        h.abort();
    }
    *is_running = false;
    *agent_rx = None;
    *was_reasoning = false;

    // 2. Record progress so far. `turn_trace` is a capped/truncated digest, so
    // this is best-effort continuity, paired with any partial response text.
    let mut recap = String::new();
    if !response_buf.trim().is_empty() {
        recap.push_str(response_buf.trim());
        recap.push_str("\n\n");
    }
    if !turn_trace.is_empty() {
        recap.push_str("[Progress this turn before context compaction]\n");
        for line in turn_trace.iter() {
            recap.push_str(line);
            recap.push('\n');
        }
    }
    let recap = recap.trim();
    if !recap.is_empty() {
        session.add_message(MessageRole::Assistant, recap);
    }
    turn_trace.clear();
    response_buf.clear();
    *response_start_line = None;
    *agent_line_started = false;

    // Unlike the between-turn gate, this announces unconditionally: the relief
    // here is dropping the aborted run's in-flight tool context via the respawn
    // below, which always happens even when the `handle_compress` step is a
    // no-op. So the message describes the restart rather than promising a
    // summarize step (which may not run and would otherwise leave the user
    // waiting on a "compressed N messages" line that never comes).
    renderer.write_line(
        &format!(
            "mid-turn context relief, restarting (at {}%)...",
            (pressure * 100.0).round() as u64
        ),
        Color::DarkGrey,
    )?;

    // 3. Compact the session (no-op if its text history is under the limit).
    #[cfg(feature = "mcp")]
    let mcp_ref = ensure_mcp_manager(mcp_manager, cfg).await;
    let compress_result = handle_compress(
        None,
        true,
        agent,
        client,
        renderer,
        session,
        cli,
        cfg,
        context,
        reasoning_enabled,
        permission,
        ask_tx,
        sandbox,
        #[cfg(feature = "mcp")]
        mcp_ref,
    )
    .await;
    if let Err(e) = compress_result {
        renderer.write_line(&format!("mid-turn compact error: {}", e), C_ERROR)?;
    }

    // 4. Respawn on the compacted history with the continuation prompt.
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
        .spawn_runner(
            MID_TURN_CONTINUE_PROMPT.to_string(),
            history,
            cfg.retry.clone(),
            #[cfg(feature = "hooks")]
            None,
        )
        .await;
    *agent_rx = Some(runner.event_rx);
    *main_abort = Some(runner.abort_handle);
    *is_running = true;
    if let Some(ss) = status_signals.as_ref() {
        ss.send_start();
    }
    Ok(())
}

/// Hard stop for a turn whose context cannot be brought under the mid-turn
/// ceiling even after a compaction. What remains is the irreducible floor
/// (system prompt, tool schemas, kept-recent transcript, reserved response
/// space), so compacting again is futile. Aborts the run and shows the user the
/// full arithmetic — the model and context-window combination is simply too
/// small to run the agentic loop on this task.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stop_turn_context_exhausted(
    prompt_tokens: u64,
    threshold: f64,
    renderer: &mut Renderer,
    session: &Session,
    cfg: &Config,
    agent_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
    main_abort: &mut Option<tokio::task::AbortHandle>,
    is_running: &mut bool,
    status_signals: &Option<StatusSignals>,
    turn_trace: &mut Vec<compact_str::CompactString>,
    response_buf: &mut String,
    response_start_line: &mut Option<usize>,
    agent_line_started: &mut bool,
    was_reasoning: &mut bool,
) -> anyhow::Result<()> {
    if let Some(h) = main_abort.take() {
        h.abort();
    }
    *is_running = false;
    *agent_rx = None;
    *was_reasoning = false;
    *agent_line_started = false;
    turn_trace.clear();
    response_buf.clear();
    *response_start_line = None;
    if let Some(ss) = status_signals.as_ref() {
        ss.send_stop();
    }

    renderer.write_line("error: not enough context to continue this turn.", C_ERROR)?;
    renderer.write_line(
        "Compaction ran, but the next prompt was still over the mid-turn ceiling. \
         Compacting again cannot help: what remains is the irreducible floor (system \
         prompt, tool schemas, the kept-recent transcript, and reserved response \
         space). Stopping the turn so the conversation is not corrupted.",
        Color::White,
    )?;
    renderer.write_line("", Color::White)?;
    for line in context_exhausted_report(
        prompt_tokens,
        threshold,
        session.context_window,
        cfg.resolve_reserve_tokens(&session.model, &crate::config::quick_models_map(cfg)),
        cfg.resolve_keep_recent_tokens(),
    ) {
        renderer.write_line(&line, Color::White)?;
    }
    Ok(())
}

/// Builds the math-and-guidance body for a context-exhaustion stop. Pure (no
/// I/O) so the arithmetic can be unit-tested. `window` must be non-zero (the
/// caller only reaches here after gating on `context_window > 0`).
pub(crate) fn context_exhausted_report(
    prompt_tokens: u64,
    threshold: f64,
    window: u64,
    reserve: u64,
    keep_recent: u64,
) -> Vec<String> {
    let ceiling = (threshold * window as f64) as u64;
    let pressure_pct = prompt_tokens as f64 / window as f64 * 100.0;
    let overflow = prompt_tokens.saturating_sub(ceiling);
    vec![
        format!("  context window .............. {window} tokens"),
        format!(
            "  mid-turn ceiling ............ {ceiling} tokens  ({:.0}% of window)",
            threshold * 100.0
        ),
        format!(
            "  prompt after compaction ..... {prompt_tokens} tokens  ({pressure_pct:.0}% of window)"
        ),
        format!("  overflow above ceiling ...... {overflow} tokens"),
        format!("  reserved for response ....... {reserve} tokens"),
        format!("  kept-recent budget .......... {keep_recent} tokens"),
        String::new(),
        "This model and context-window combination is too small to run zerostack's \
         agentic loop on this task. To proceed you can:"
            .to_string(),
        "  - increase context_window (and the model server's real KV cache) so the \
         window clears the floor above;"
            .to_string(),
        format!(
            "  - raise mid_turn_compact_threshold above {pressure_pct:.0}% so this prompt \
             fits under the ceiling (trades safety for room: the real KV cache must still \
             hold {prompt_tokens}+ tokens);"
        ),
        "  - lower keep_recent_tokens or reserve_tokens to shrink the floor;".to_string(),
        "  - switch to a model/server with a larger context window, or split the task \
         into smaller pieces."
            .to_string(),
    ]
}

#[allow(clippy::too_many_arguments)]
pub async fn run_interactive(
    client: AnyClient,
    agent: Option<AnyAgent>,
    cli: &Cli,
    cfg: &Config,
    session: &mut Session,
    context: &mut ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    ask_rx: Option<AskReceiver>,
    sandbox: Sandbox,
    auto_trigger_msg: Option<String>,
    status_signals: Option<StatusSignals>,
    #[cfg(feature = "advisor")] handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,
) -> anyhow::Result<()> {
    let mut app = app::App::new(
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
        auto_trigger_msg,
        status_signals,
        #[cfg(feature = "advisor")]
        handoff_rx,
    )
    .await?;
    app.run().await?;
    app.teardown().await;
    Ok(())
}
#[cfg(feature = "advisor")]
pub(crate) async fn handle_human_handoff(
    req: crate::extras::advisor::HandoffRequest,
    renderer: &mut Renderer,
    user_rx: &mut mpsc::Receiver<UserEvent>,
    agent_line_started: &mut bool,
    was_reasoning: &mut bool,
) -> anyhow::Result<()> {
    *was_reasoning = false;
    if *agent_line_started {
        renderer.write_line("", Color::White)?;
        *agent_line_started = false;
    }

    renderer.write_line("[handoff] Model requests your guidance:", C_HANDOFF)?;
    for line in req.question.lines() {
        renderer.write_line(&format!("  | {}", sanitize_output(line)), C_HANDOFF)?;
    }
    renderer.write_line("", C_HANDOFF)?;
    renderer.write_line(
        "  Type your response and press Enter (ESC to cancel):",
        C_HANDOFF,
    )?;

    let mut buffer = String::new();
    let response = loop {
        tokio::select! {
            Some(ev) = user_rx.recv() => {
                if let crate::event::UserEvent::Key(key) = ev {
                    match key.code {
                        crossterm::event::KeyCode::Enter => break buffer,
                        crossterm::event::KeyCode::Esc => break String::new(),
                        crossterm::event::KeyCode::Char(c) => {
                            buffer.push(c);
                            renderer.write_line(&format!("  > {}", buffer), C_HANDOFF)?;
                        }
                        crossterm::event::KeyCode::Backspace => {
                            buffer.pop();
                            renderer.write_line(&format!("  > {}", buffer), C_HANDOFF)?;
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    if response.is_empty() {
        renderer.write_line("  [cancelled]", C_HANDOFF)?;
    } else {
        renderer.write_line(&format!("  [sent: {}]", response), C_HANDOFF)?;
    }
    renderer.write_line("", Color::White)?;

    let _ = req.reply.send(response);
    Ok(())
}
