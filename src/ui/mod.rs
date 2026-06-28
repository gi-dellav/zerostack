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
use crate::extras::status_signals::StatusSignals;
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
use crate::ui::pickers::rewind::RewindOutcome;
use crate::ui::renderer::{Renderer, copy_to_clipboard};
use crate::ui::slash::{handle_compress, handle_slash};
use crate::ui::terminal::TerminalGuard;

use self::utils::parse_color;

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
fn refresh_display(
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
async fn spawn_merge_agent(
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
        .spawn_runner(prompt, history);
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
type PrebuildPayload = (AnyAgent, Option<McpClientManager>);
#[cfg(not(feature = "mcp"))]
type PrebuildPayload = AnyAgent;

/// If the background prebuild hasn't delivered yet, block until it does.
#[cfg(feature = "mcp")]
async fn resolve_prebuild<'a>(
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
fn resolve_prebuild<'a>(
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
        .spawn_runner(text.to_string(), history);
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
async fn mid_turn_compact_and_respawn(
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
        .spawn_runner(MID_TURN_CONTINUE_PROMPT.to_string(), history);
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
fn stop_turn_context_exhausted(
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
    status_signals: Option<StatusSignals>,
    #[cfg(feature = "advisor")] mut handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,
) -> anyhow::Result<()> {
    let _guard = TerminalGuard::new()?;

    // Display preference: whether the status bar shows the cost even at $0.0000.
    session.show_cost_always = cfg.resolve_show_cost_always();
    // Statusline layout: parse the spec once, size the renderer's statusline rows.
    crate::ui::statusline::init(cfg);

    // Status-bar git data: seed now, then refresh on a throttle in the loop
    // (covers worktree switches and external `git checkout` without per-render IO).
    session.refresh_git_branch();
    if crate::ui::statusline::needs_git_status() {
        session.refresh_git_status();
    }
    let mut last_branch_check = std::time::Instant::now();

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
    let mut show_reasoning = cfg.resolve_show_reasoning();
    let mut reasoning_enabled = true;
    session.reasoning_enabled = reasoning_enabled;
    // Seed the context-overhead estimate so the status bar reflects the system
    // prompt + context files from T0, before the first request is calibrated.
    // `ensure_agent` refreshes this whenever the preamble is rebuilt.
    session.overhead_tokens = crate::agent::builder::estimate_overhead(context, reasoning_enabled);
    // Text of the in-flight interactive send. Set by `start_main_run`; on a
    // failed turn it is rolled back into the input editor so the message never
    // poisons the session (which would 400 forever until a manual `/undo`).
    let mut pending_send: Option<String> = None;
    let mut was_reasoning = false;
    let mut todo_tools_enabled = false;
    #[allow(unused_mut)]
    let mut loop_label: Option<String> = None;
    #[cfg(feature = "loop")]
    let mut loop_state: Option<crate::extras::r#loop::LoopState> = None;
    #[cfg(feature = "git-worktree")]
    let mut wt_return_path: Option<(String, String, String, bool)> = None;
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
    // True between a mid-turn compaction and the next provider call. If that
    // call is *still* over the ceiling, compaction failed to free space and the
    // turn is stopped (see `stop_turn_context_exhausted`); if it comes back
    // under, relief worked and the flag clears so a later accumulation can
    // compact again. Reset at every turn boundary.
    let mut awaiting_compaction_relief = false;
    let mut dot_prompt_restore: Option<String> = None;
    let mut chain_pending: Option<crate::extras::chain::ChainPhase> = None;
    let mut chain_label_msg: Option<String> = None;

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
        chain_label_msg.as_deref(),
        btw_total_cost,
        btw_total_in,
        btw_total_out,
    )?;

    // pre-warm the current provider's live models into the picker (best-effort)
    // Moved after first paint so the TUI is visible while the network call completes.
    {
        let provider = session.provider.to_string();
        let is_custom = cfg.custom_providers_map().contains_key(&provider);
        let ids = crate::ui::slash::warm_model_cache(&provider, is_custom, &client, cli, cfg).await;
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
        if let Some(ss) = status_signals.as_ref() {
            ss.send_start();
        }
        session.add_message(MessageRole::User, trigger_msg);
        #[cfg(feature = "advisor")]
        crate::extras::advisor::set_session_messages(session.messages.clone());
    }

    let (mut user_tx, mut user_rx) = mpsc::channel::<UserEvent>(64);
    let mut running = Arc::new(AtomicBool::new(true));
    let mut event_handle = Some(spawn_event_thread(user_tx.clone(), running.clone()));

    // Prebuild the agent on a background task so the TUI is responsive from the
    // first frame. The event thread is already buffering keystrokes; if the user
    // submits before the build completes we wait for it in `start_main_run`.
    let (prebuild_tx, prebuild_rx_raw) = mpsc::channel::<PrebuildPayload>(1);
    let mut prebuild_rx = Some(prebuild_rx_raw);
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

    loop {
        session.reasoning_enabled = reasoning_enabled;
        if last_branch_check.elapsed() >= std::time::Duration::from_secs(1) {
            session.refresh_git_branch();
            if crate::ui::statusline::needs_git_status() {
                session.refresh_git_status();
            }
            last_branch_check = std::time::Instant::now();
        }
        tokio::select! {
            Some(ev) = user_rx.recv() => {
                match ev {
                    UserEvent::Resize => {
                        renderer.resize();
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::ScrollUp => {
                        // Scroll the input viewport first; once it is at the top,
                        // keep wheeling up to scroll back through the chat history.
                        if !renderer.input_scroll_up() {
                            renderer.scroll_line_up();
                        }
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::ScrollDown => {
                        // Bring the chat history back down first, then the input.
                        if renderer.is_scrolling() {
                            renderer.scroll_line_down();
                        } else {
                            renderer.input_scroll_down();
                        }
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    UserEvent::MouseDown { row, col } => {
                        // A click inside the input area moves the cursor there;
                        // otherwise it starts a chat-history text selection.
                        if let Some(pos) = renderer.input_cursor_for_click(row, col, &input.buffer) {
                            input.set_cursor(pos);
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        } else if row < renderer.visible_lines() as u16
                            && let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_active = true;
                                renderer.selection_start = Some(idx);
                                renderer.selection_end = Some(idx);
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            }
                        continue;
                    }
                    UserEvent::MouseDrag { row, col: _ } => {
                        if renderer.selection_active
                            && let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_end = Some(idx);
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        }
                        continue;
                    }
                    UserEvent::Paste(data) => {
                        input.handle_paste(data);
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    }
                    #[cfg(feature = "mcp")]
                    UserEvent::McpLoginDone { server, error } => {
                        if let Some(err) = error {
                            renderer.write_line(&format!("login failed for '{}': {}", server, err), C_ERROR)?;
                        } else {
                            let server = server.to_string();
                            let server_cfg = cfg.mcp_servers.as_ref().and_then(|m| m.get(&server).cloned());
                            match (mcp_manager.as_mut(), server_cfg) {
                                (Some(mgr), Some(scfg)) => {
                                    match mgr.reconnect(&server, &scfg).await {
                                        Ok(()) => {
                                            let model = client.completion_model(session.model.to_string());
                                            let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
                                            let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
                                            agent = Some(crate::provider::build_agent(
                                                model, cli, cfg, context,
                                                permission.clone(), ask_tx.clone(), sandbox.clone(),
                                                reasoning_enabled, temperature, extra_body,
                                                mcp_manager.as_ref(),
                                            ).await);
                                            renderer.write_line(&format!("authorized and connected '{}'", server), C_AGENT)?;
                                        }
                                        Err(err) => {
                                            renderer.write_line(&format!("authorized '{}' but reconnect failed: {}", server, err), C_ERROR)?;
                                        }
                                    }
                                }
                                _ => {
                                    renderer.write_line(&format!("authorized '{}' (will connect on next start)", server), C_AGENT)?;
                                }
                            }
                        }
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            } else if is_running {
                                // Actually cancel the run's task (not just stop
                                // listening), so it stops executing tools. bash
                                // children are killed via kill_on_drop.
                                if let Some(h) = main_abort.take() {
                                    h.abort();
                                }
                                sandbox.kill_active();
                                is_running = false;
                                if let Some(ss) = status_signals.as_ref() {
                                    ss.send_stop();
                                }
                                agent_rx = None;
                                turn_trace.clear();
                                awaiting_compaction_relief = false;
                                pending_inputs.clear();
                                #[cfg(feature = "loop")]
                                if let Some(ref mut ls) = loop_state {
                                    ls.active = false;
                                    loop_label = None;
                                }
                                if !input.buffer.is_empty() {
                                    input.clear_buffer();
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
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }
                        if renderer.selection_active && key.code == KeyCode::Esc {
                            renderer.clear_selection();
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        match key.code {
                            KeyCode::PageUp => {
                                renderer.scroll_page_up();
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            KeyCode::PageDown => {
                                renderer.scroll_page_down();
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            KeyCode::Home => {
                                renderer.scroll_to_top();
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            KeyCode::End => {
                                renderer.scroll_to_bottom()?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            _ => {}
                        }

                        if input.picker.as_ref().is_some_and(|p| p.active())
                            && input.handle_picker_key(key) {
                                // The rewind picker resolves through here: once it
                                // confirms, perform the rewind on the live session
                                // and drop the chosen turn back into the input box.
                                if let Some(RewindOutcome::Confirmed(idx)) =
                                    input.take_rewind_outcome()
                                {
                                    let text =
                                        session.messages.get(idx).map(|m| m.content.to_string());
                                    if session.rewind_to(idx) > 0 {
                                        if let Some(text) = text {
                                            input.load_text(&text);
                                        }
                                        if !cli.no_session {
                                            let _ = crate::session::storage::save_session(session);
                                        }
                                        render_session(&mut renderer, session, cli, cfg, context)?;
                                        renderer.write_line(
                                            "rewound; /redo to restore",
                                            Color::Green,
                                        )?;
                                    }
                                }
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        // Chain prompt active: intercept Y/N/B keystrokes
                        if renderer.chain_prompt.is_some() && !renderer.chain_but_mode {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    renderer.chain_prompt = None;
                                    if let Some(phase) = chain_pending.take() {
                                        chain_label_msg = None;
                                        let next_name = phase.next_prompt_name();
                                        if let Some(content) = context.prompts.get(next_name).cloned() {
                                            let (mode_directive_str, clean_content) =
                                                crate::permission::parse_prompt_mode(&content);
                                            let mode_directive = mode_directive_str.map(|s| s.to_string());
                                            context.current_prompt = Some(if mode_directive.is_some() {
                                                clean_content.to_string()
                                            } else {
                                                content
                                            });
                                            context.current_prompt_name = Some(next_name.to_string());
                                            if let Some(ref mode_str) = mode_directive {
                                                if mode_str == "last_user_mode"
                                                    && let Some(perm) = &permission
                                                {
                                                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                                    guard.restore_user_mode();
                                                } else if let Some(mode) =
                                                    crate::permission::SecurityMode::from_str(mode_str)
                                                    && let Some(perm) = &permission
                                                {
                                                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                                    guard.set_prompt_mode(mode);
                                                }
                                            }
                                        }
                                        let msg = phase.transition_message().to_string();
                                        for line in msg.lines() {
                                            renderer.write_line(
                                                &format!("> {}", sanitize_output(line)),
                                                Color::Green,
                                            )?;
                                        }
                                        renderer.write_line("", Color::White)?;
                                        session.add_message(MessageRole::User, &msg);
                                        agent = None;
                                        start_main_run(
                                            &msg, &mut agent, &client, session, cli,
                                            cfg, context, &permission, &ask_tx, &sandbox,
                                            reasoning_enabled, &mut agent_rx,
                                            &mut main_abort, &mut is_running,
                                            &status_signals,
                                            #[cfg(feature = "mcp")] &mut mcp_manager,
                                            &mut prebuild_rx,
                                            &mut pending_send,
                                        ).await;
                                    }
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') => {
                                    renderer.chain_prompt = None;
                                    chain_pending = None;
                                    chain_label_msg = None;
                                    renderer.write_line(
                                        "chain declined — won't ask again this session",
                                        C_AGENT,
                                    )?;
                                    if let Some(ref name) = context.current_prompt_name
                                        && !context.chain_declined.contains(name)
                                    {
                                        context.chain_declined.push(name.clone());
                                    }
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                KeyCode::Char('b') | KeyCode::Char('B') => {
                                    renderer.chain_but_mode = true;
                                    renderer.chain_prompt = None;
                                    input.clear_buffer();
                                    chain_label_msg = chain_pending.map(|p| p.chain_label().to_string());
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                _ => {
                                    // Ignore other keystrokes while chain prompt is active
                                    continue;
                                }
                            }
                        }
                        // Chain but mode: Esc cancels back to ask
                        if renderer.chain_but_mode && key.code == KeyCode::Esc {
                            renderer.chain_but_mode = false;
                            if let Some(phase) = chain_pending {
                                renderer.chain_prompt = Some(renderer::ChainPrompt {
                                    question: compact_str::CompactString::from(phase.chain_label()),
                                });
                                chain_label_msg = Some(phase.chain_label().to_string());
                            }
                            input.clear_buffer();
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                            continue;
                        }

                        if let Some(mut text) = input.handle_key(key) {
                            #[cfg(feature = "loop")]
                            if loop_state.as_ref().is_some_and(|ls| ls.active) && !text.starts_with('/') {
                                renderer.write_line("loop active: /loop stop to cancel", C_ERROR)?;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
                            }
                            if renderer.is_scrolling() {
                                renderer.scroll_to_bottom()?;
                            }
                            // Chain-of-prompts: handle text submission after B (but) mode
                            if !is_running
                                && let Some(phase) = chain_pending.take()
                            {
                                chain_label_msg = None;
                                renderer.chain_but_mode = false;
                                let trimmed = text.trim().to_string();
                                if trimmed.is_empty() {
                                    // Empty but — restore ask prompt
                                    chain_pending = Some(phase);
                                    chain_label_msg = Some(phase.chain_label().to_string());
                                    renderer.chain_prompt = Some(renderer::ChainPrompt {
                                        question: compact_str::CompactString::from(phase.chain_label()),
                                    });
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                // Accept with extra instruction
                                let next_name = phase.next_prompt_name();
                                if let Some(content) = context.prompts.get(next_name).cloned() {
                                    let (mode_directive_str, clean_content) =
                                        crate::permission::parse_prompt_mode(&content);
                                    let mode_directive = mode_directive_str.map(|s| s.to_string());
                                    context.current_prompt = Some(if mode_directive.is_some() {
                                        clean_content.to_string()
                                    } else {
                                        content
                                    });
                                    context.current_prompt_name = Some(next_name.to_string());
                                    if let Some(ref mode_str) = mode_directive {
                                        if mode_str == "last_user_mode"
                                            && let Some(perm) = &permission
                                        {
                                            let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                            guard.restore_user_mode();
                                        } else if let Some(mode) =
                                            crate::permission::SecurityMode::from_str(mode_str)
                                            && let Some(perm) = &permission
                                        {
                                            let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                            guard.set_prompt_mode(mode);
                                        }
                                    }
                                }
                                let base_msg = phase.transition_message().to_string();
                                let msg = format!("{}\n\nAdditional instructions: {}", base_msg, trimmed);
                                for line in msg.lines() {
                                    renderer.write_line(
                                        &format!("> {}", sanitize_output(line)),
                                        Color::Green,
                                    )?;
                                }
                                renderer.write_line("", Color::White)?;
                                session.add_message(MessageRole::User, &msg);
                                agent = None;
                                start_main_run(
                                    &msg, &mut agent, &client, session, cli,
                                    cfg, context, &permission, &ask_tx, &sandbox,
                                    reasoning_enabled, &mut agent_rx,
                                    &mut main_abort, &mut is_running,
                                    &status_signals,
                                    #[cfg(feature = "mcp")] &mut mcp_manager,
                                    &mut prebuild_rx,
                                    &mut pending_send,
                                ).await;
                                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                continue;
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
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                SubmitAction::RejectWhileRunning => {
                                    renderer.write_line(
                                        "agent is running — wait for it to finish or press Ctrl-C before running a command",
                                        C_ERROR,
                                    )?;
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                                    continue;
                                }
                                SubmitAction::Queue => {
                                    pending_inputs.push_back(text.to_string());
                                    renderer.write_line(&format!("queued: {}", sanitize_output(&text)), C_TOOL)?;
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                                        let temperature =
                                            crate::config::resolve_temperature(cli, cfg, &session.model);
                                        let extra_body =
                                            crate::config::resolve_extra_body(cfg, &session.model);
                                        let btw_agent = crate::provider::build_btw_agent(
                                            model, cli, cfg, context, &permission, &ask_tx, reasoning_enabled, temperature, extra_body,
                                        );
                                        let runner = btw_agent.spawn_btw(
                                            btw_text.to_string(), snapshot, btw_tx.clone(), id,
                                        );
                                        btw_abort.push((id, runner.abort_handle));
                                        btw_inflight += 1;
                                        renderer.write_line(&format!("[btw #{}] thinking...", id), C_BTW)?;
                                    }
                                    refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                                            if let Some(ref mode_str) = mode_directive
                                                && let Some(perm) = &permission {
                                                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                                    if mode_str == "last_user_mode" {
                                                        guard.restore_user_mode();
                                                    } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str) {
                                                        guard.set_prompt_mode(mode);
                                                    }
                                                }
                                        }
                                        text = msg.to_string().into();
                                        is_dot_cmd = false;
                                        agent = None;
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
                                            if let Some(ref mode_str) = mode_directive
                                                && let Some(perm) = &permission {
                                                    let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
                                                    if mode_str == "last_user_mode" {
                                                        guard.restore_user_mode();
                                                    } else if let Some(mode) = crate::permission::SecurityMode::from_str(mode_str) {
                                                        guard.set_prompt_mode(mode);
                                                    }
                                                }
                                        }
                                        agent = None;
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
                                            false,
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
                                    #[cfg(feature = "mcp")]
                                    Err(e) if e.to_string().starts_with(crate::ui::slash::settings::DEFER_MCP_LOGIN) => {
                                        let server = e.to_string()
                                            .strip_prefix(crate::ui::slash::settings::DEFER_MCP_LOGIN)
                                            .unwrap_or_default()
                                            .trim()
                                            .to_string();
                                        // Re-resolve the URL server + OAuth settings from config.
                                        let resolved = cfg.mcp_servers.as_ref().and_then(|m| m.get(&server)).and_then(|s| {
                                            if let crate::extras::mcp::config::McpServerConfig::Url { url, oauth, .. } = s {
                                                oauth.as_ref().and_then(|o| o.settings()).map(|set| (url.clone(), set))
                                            } else {
                                                None
                                            }
                                        });
                                        match resolved {
                                            Some((url, settings)) => {
                                                renderer.write_line(&format!("starting OAuth login for '{}'...", server), C_AGENT)?;
                                                match crate::extras::mcp::oauth::begin_login(&server, &url, &settings).await {
                                                    Ok(login) => {
                                                        // Auto-copy so the user can paste it; print it on its
                                                        // own line so terminals linkify it and it selects cleanly.
                                                        copy_to_clipboard(&login.auth_url);
                                                        renderer.write_line("open this URL to authorize (copied to clipboard):", C_AGENT)?;
                                                        renderer.write_line(&login.auth_url, Color::Cyan)?;
                                                        renderer.write_line(
                                                            &format!("waiting for authorization on 127.0.0.1:{} in the background...", settings.redirect_port()),
                                                            Color::DarkGrey,
                                                        )?;
                                                        // Run the (long) browser wait off the event loop so the
                                                        // TUI stays responsive; report back via UserEvent.
                                                        let tx = user_tx.clone();
                                                        let sname = compact_str::CompactString::new(&server);
                                                        tokio::spawn(async move {
                                                            let error = login
                                                                .wait_for_callback(std::time::Duration::from_secs(180))
                                                                .await
                                                                .err()
                                                                .map(|e| compact_str::CompactString::new(e.to_string()));
                                                            let _ = tx.send(crate::event::UserEvent::McpLoginDone { server: sname, error }).await;
                                                        });
                                                    }
                                                    Err(err) => {
                                                        renderer.write_line(&format!("login setup failed for '{}': {}", server, err), C_ERROR)?;
                                                    }
                                                }
                                            }
                                            None => {
                                                renderer.write_line(&format!("cannot start login for '{}' (not an OAuth URL server)", server), C_ERROR)?;
                                            }
                                        }
                                    }
                                    #[cfg(feature = "git-worktree")]
                                    Err(e) if e.downcast_ref::<crate::extras::git_worktree::DeferredWorktreeAction>().is_some() => {
                                        let action = e.downcast_ref::<crate::extras::git_worktree::DeferredWorktreeAction>().unwrap();
                                        match action {
                                            crate::extras::git_worktree::DeferredWorktreeAction::Merge { branch, target, main_path, wt_path } => {
                                                #[cfg(feature = "git-worktree")]
                                                let force_flag = cli.resolve_wt_force(cfg);
                                                #[cfg(not(feature = "git-worktree"))]
                                                let force_flag = false;
                                                spawn_merge_agent(
                                                    branch, target, main_path, wt_path,
                                                    force_flag,
                                                    session,
                                                    &mut agent, &client, cli, cfg, context,
                                                    &permission, &ask_tx, &sandbox, reasoning_enabled,
                                                    &mut agent_rx, &mut main_abort, &mut is_running,
                                                    &status_signals,
                                                    &mut wt_return_path,
                                                    #[cfg(feature = "mcp")] &mut mcp_manager,
                                                ).await;
                                            }
                                            crate::extras::git_worktree::DeferredWorktreeAction::Exit { main_path } => {
                                                std::env::set_current_dir(main_path)
                                                    .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
                                                session.working_dir = compact_str::CompactString::new(main_path);
                                                context.reload();
                                                apply_current_prompt_mode(context, &permission);
                                                #[cfg(feature = "mcp")]
                                                let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                                let model = client.completion_model(session.model.to_string());
                                                let temperature =
                                                    crate::config::resolve_temperature(cli, cfg, &session.model);
                                                let extra_body =
                                                    crate::config::resolve_extra_body(cfg, &session.model);
                                                agent = Some(crate::provider::build_agent(
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
                                                    #[cfg(feature = "mcp")] mcp_ref,
                                                ).await);
                                                render_session(&mut renderer, session, cli, cfg, context)?;
                                                renderer.write_line(
                                                    &format!("returned to main repo at {}", main_path),
                                                    C_AGENT,
                                                )?;
                                            }
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
                                        if let Some(ss) = status_signals.as_ref() {
                                            ss.send_start();
                                        }
                                    }
                                    Err(e) if e.to_string().starts_with("DEFER_REVIEW:") => {
                                        let msg = e.to_string().strip_prefix("DEFER_REVIEW:").unwrap_or("").to_string();
                                        dot_prompt_restore = context.one_shot_restore.take();
                                        session.add_message(MessageRole::User, &msg);
                                        #[cfg(feature = "mcp")]
                                        let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                        ensure_agent(
                                            &mut agent, &client, session, cli, cfg, context,
                                            &permission, &ask_tx, &sandbox, reasoning_enabled,
                                            #[cfg(feature = "mcp")] mcp_ref,
                                        ).await;
                                        let history = crate::agent::runner::convert_history(session);
                                        let runner = agent.as_ref().unwrap().clone().spawn_runner(msg, history);
                                        agent_rx = Some(runner.event_rx);
                                        main_abort = Some(runner.abort_handle);
                                        is_running = true;
                                        if let Some(ss) = status_signals.as_ref() {
                                            ss.send_start();
                                        }
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
                                    &status_signals,
                                    #[cfg(feature = "mcp")] &mut mcp_manager,
                                    &mut prebuild_rx,
                                    &mut pending_send,
                                ).await;
                            }
                            }
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        } else {
                            refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        }
                    }
                }
            }
            // Consume the background prebuild as soon as it is ready (while idle)
            // so MCP connection notices render in the transcript instead of the
            // prebuild's stderr logging racing against the alt-screen TUI.
            Some(prebuilt) = async { prebuild_rx.as_mut()?.recv().await }, if agent.is_none() => {
                #[cfg(feature = "mcp")]
                {
                    let (built_agent, built_mcp) = prebuilt;
                    agent = Some(built_agent);
                    mcp_manager = built_mcp;
                    if let Some(m) = mcp_manager.as_mut() {
                        for notice in m.take_notices() {
                            renderer.write_line(&notice, C_ERROR)?;
                        }
                    }
                }
                #[cfg(not(feature = "mcp"))]
                {
                    agent = Some(prebuilt);
                }
                prebuild_rx = None;
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                continue;
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
                    AgentEvent::Done { .. } | AgentEvent::Error(_) => {
                        turn_trace.clear();
                        awaiting_compaction_relief = false;
                    }
                    _ => {}
                }
                // Mid-turn compaction (PR H). On a provider-call boundary, if
                // real prompt pressure crossed the opt-in threshold, abort the
                // run cleanly, compact, and respawn on the compacted history.
                // Gated by `compact_enabled` (master switch) and suppressed in
                // /loop runs and `--no-session` mode (which never compact).
                #[cfg(feature = "loop")]
                let loop_running = loop_state.as_ref().is_some_and(|ls| ls.active);
                #[cfg(not(feature = "loop"))]
                let loop_running = false;
                if let AgentEvent::CompletionCall {
                    input_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                    ..
                } = &event
                    && is_running
                    && !loop_running
                    && !cli.no_session
                    && cfg.resolve_compact_enabled()
                    && session.context_window > 0
                    && let Some(threshold) = cfg.resolve_mid_turn_compact_threshold()
                {
                    // Use the cache-inclusive prompt size so Anthropic cache hits
                    // (input_tokens excludes cached/cache-creation) don't understate
                    // real context pressure and suppress mid-turn compaction.
                    let real_input_tokens = crate::session::Session::real_input_tokens(
                        cfg.is_anthropic_native(&session.provider),
                        *input_tokens,
                        *cached_input_tokens,
                        *cache_creation_input_tokens,
                    );
                    let pressure = real_input_tokens as f64 / session.context_window as f64;
                    if pressure > threshold {
                        if awaiting_compaction_relief {
                            // We already compacted this turn and the very next
                            // provider call is STILL over the ceiling — the floor
                            // exceeds the budget, so compacting again is futile.
                            // Stop the turn and show the user the arithmetic.
                            stop_turn_context_exhausted(
                                real_input_tokens, threshold, &mut renderer, session, cfg,
                                &mut agent_rx, &mut main_abort, &mut is_running,
                                &status_signals, &mut turn_trace, &mut response_buf,
                                &mut response_start_line, &mut agent_line_started,
                                &mut was_reasoning,
                            )?;
                            awaiting_compaction_relief = false;
                        } else {
                            mid_turn_compact_and_respawn(
                                pressure, &mut renderer, &mut agent, &mut client, session,
                                cli, cfg, context, &permission, &ask_tx, &sandbox,
                                reasoning_enabled, &mut agent_rx, &mut main_abort,
                                &mut is_running, &status_signals, &mut turn_trace,
                                &mut response_buf, &mut response_start_line,
                                &mut agent_line_started, &mut was_reasoning,
                                #[cfg(feature = "mcp")] &mut mcp_manager,
                            ).await?;
                            awaiting_compaction_relief = true;
                        }
                        refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
                        continue;
                    } else {
                        // A provider call came back under the ceiling: either we
                        // never compacted, or the compaction worked. Either way a
                        // later accumulation is allowed to compact afresh.
                        awaiting_compaction_relief = false;
                    }
                }
                #[cfg(feature = "mcp")]
                let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                // Peek before the event is consumed: a failed turn rolls the
                // in-flight interactive message back into the input editor.
                let turn_errored = matches!(&event, AgentEvent::Error(_));
                handle_agent_event(
                    event, &mut renderer, session, cfg, cli, context,
                    &mut is_running, &mut agent_rx, &mut agent_line_started,
                    &mut response_buf, &mut response_start_line, &mut was_reasoning,
                    show_reasoning,
                    &mut agent, &mut client, &mut loop_label,
                    &permission, &ask_tx, &sandbox,
                    &status_signals,
                    #[cfg(feature = "loop")] &mut loop_state,
                    #[cfg(feature = "git-worktree")] &mut wt_return_path,
                    #[cfg(feature = "mcp")] mcp_ref,
                ).await?;
                if turn_errored {
                    // The turn produced no response, so the trailing user message
                    // is still pending. Remove it (so it never poisons the next
                    // request) and restore it to the input editor for the user to
                    // edit or resend. Provider-agnostic: works for too-long, auth,
                    // rate-limit, and transient errors alike.
                    if let Some(text) = pending_send.take() {
                        let len = session.messages.len();
                        if len > 0 && session.messages[len - 1].role == MessageRole::User {
                            session.truncate_to(len - 1);
                        }
                        input.buffer = text.into();
                        input.cursor = input.buffer.len();
                    }
                } else if !is_running {
                    // Turn completed normally; the message is committed.
                    pending_send = None;
                }
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
                // Chain-of-prompts: after the agent finishes, check if the
                // current prompt is a chainable phase and trigger the prompt.
                // Skip phases that were declined earlier in this session.
                if !is_running
                    && chain_pending.is_none()
                    && let Some(ref name) = context.current_prompt_name
                    && !context.chain_declined.contains(name)
                    && let Some(phase) =
                        crate::extras::chain::ChainPhase::from_prompt_name(name)
                    && let Some(ref chain_cfg) = cfg.chain
                    && phase.is_enabled(chain_cfg)
                {
                    chain_pending = Some(phase);
                    chain_label_msg =
                        Some(phase.chain_label().to_string());
                    renderer.chain_but_mode = false;
                    renderer.chain_prompt = Some(renderer::ChainPrompt {
                        question: compact_str::CompactString::from(phase.chain_label()),
                    });
                }
                // Run finished: drop its (now-dead) abort handle and, if the user
                // queued input while it ran, replay the next one as a new run.
                if !is_running {
                    main_abort = None;
                    if let Some(next) = pending_inputs.pop_front() {
                        // Clear any chain prompt since we're starting a new run
                        renderer.chain_prompt = None;
                        renderer.chain_but_mode = false;
                        chain_pending = None;
                        chain_label_msg = None;
                        for line in next.lines() {
                            renderer.write_line(&format!("> {}", sanitize_output(line)), Color::Green)?;
                        }
                        renderer.write_line("", Color::White)?;
                        start_main_run(
                            &next, &mut agent, &client, session, cli, cfg, context,
                            &permission, &ask_tx, &sandbox, reasoning_enabled,
                            &mut agent_rx, &mut main_abort, &mut is_running,
                            &status_signals,
                            #[cfg(feature = "mcp")] &mut mcp_manager,
                            &mut prebuild_rx,
                            &mut pending_send,
                        ).await;
                    }
                }
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            Some(ask_req) = async {
                ask_rx.as_mut()?.recv().await
            } => {
                handle_permission_request(
                    ask_req, &mut renderer, session, cli,
                    &mut user_rx, &mut agent_line_started, &mut was_reasoning,
                ).await?;
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
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
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)), if is_running => {
                refresh_display(&mut renderer, &mut input, session, is_running, loop_label.as_deref(), context.current_prompt_name.as_deref(), perm_mode().as_deref(), chain_label_msg.as_deref(), btw_total_cost, btw_total_in, btw_total_out)?;
            }
            else => {
                // Poll the background prebuild; if it just completed, stash it.
                if let Some(rx) = prebuild_rx.as_mut()
                    && let Ok(payload) = rx.try_recv() {
                        #[cfg(feature = "mcp")]
                        {
                            agent = Some(payload.0);
                            mcp_manager = payload.1;
                        }
                        #[cfg(not(feature = "mcp"))]
                        {
                            agent = Some(payload);
                        }
                        prebuild_rx = None;
                    }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        }

        #[cfg(feature = "advisor")]
        if let Some(ref mut rx) = handoff_rx
            && let Ok(handoff_req) = rx.try_recv()
        {
            handle_human_handoff(
                handoff_req,
                &mut renderer,
                &mut user_rx,
                &mut agent_line_started,
                &mut was_reasoning,
            )
            .await?;
            refresh_display(
                &mut renderer,
                &mut input,
                session,
                is_running,
                loop_label.as_deref(),
                context.current_prompt_name.as_deref(),
                perm_mode().as_deref(),
                chain_label_msg.as_deref(),
                btw_total_cost,
                btw_total_in,
                btw_total_out,
            )?;
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
        let mut proceed = true;
        if crate::extras::git_worktree::worktree_has_uncommitted(&info.worktree_path) {
            let _ = renderer.write_line(
                "worktree has uncommitted changes. [c]ommit all and continue  [a]bort merge",
                C_PERM,
            );
            if let Some(ss) = status_signals.as_ref() {
                ss.send_git_conflict();
            }
            let action = loop {
                tokio::select! {
                    Some(ev) = user_rx.recv() => {
                        if let crate::event::UserEvent::Key(key) = ev {
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
                        let _ = renderer.write_line(&format!("auto-commit failed: {}", e), C_ERROR);
                        proceed = false;
                    } else {
                        let _ = renderer.write_line(
                            "committed all worktree changes, proceeding with merge",
                            C_AGENT,
                        );
                    }
                }
                'a' => {
                    let _ = renderer.write_line("merge aborted, worktree left untouched", C_AGENT);
                    proceed = false;
                }
                _ => unreachable!(),
            }
        }
        let (state, outcome) = if proceed {
            crate::extras::git_worktree::try_merge(&info, &target)
        } else {
            // Skip merge; outcome is unused, construct a dummy state.
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
                if let Some(ss) = status_signals.as_ref() {
                    ss.send_git_conflict();
                }
                let _ = renderer.write_line(
                    "[a]bort  [l]eave for manual resolution  [h]elp (agent resolves)",
                    C_PERM,
                );

                let action = loop {
                    tokio::select! {
                        Some(ev) = user_rx.recv() => {
                            if let crate::event::UserEvent::Key(key) = ev {
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
                            cli.resolve_wt_force(cfg),
                        );
                        let _ =
                            renderer.write_line("merge aborted, restored original state", C_AGENT);
                    }
                    'l' => {
                        let _ = renderer.write_line(
                            &format!(
                                "conflict state left in {} for manual resolution",
                                info.main_repo_path.display()
                            ),
                            C_AGENT,
                        );
                    }
                    'h' => {
                        let _ = crate::extras::git_worktree::cancel_merge(&state);
                        let _ = renderer.write_line("agent resolving merge...", C_AGENT);
                        let main_path = info.main_repo_path.display().to_string();
                        let wt_path = info.worktree_path.display().to_string();
                        let force_flag = cli.resolve_wt_force(cfg);
                        spawn_merge_agent(
                            &info.branch,
                            &target,
                            &main_path,
                            &wt_path,
                            force_flag,
                            session,
                            &mut agent,
                            &client,
                            cli,
                            cfg,
                            context,
                            &permission,
                            &ask_tx,
                            &sandbox,
                            reasoning_enabled,
                            &mut agent_rx,
                            &mut main_abort,
                            &mut is_running,
                            &status_signals,
                            &mut wt_return_path,
                            #[cfg(feature = "mcp")]
                            &mut mcp_manager,
                        )
                        .await;

                        let mut agent_line_started = false;
                        let mut merge_response_buf = String::new();
                        let mut merge_response_start_line = None;
                        let mut merge_was_reasoning = false;
                        while is_running {
                            let ev = match agent_rx.as_mut() {
                                Some(rx) => {
                                    tokio::select! {
                                        Some(e) = rx.recv() => Some(e),
                                        Some(ev) = user_rx.recv() => {
                                            if let crate::event::UserEvent::Key(key) = ev {
                                                let is_ctrl_c = key.code == KeyCode::Char('c')
                                                    && key.modifiers.contains(KeyModifiers::CONTROL);
                                                if is_ctrl_c {
                                                    if let Some(h) = main_abort.take() {
                                                        h.abort();
                                                    }
                                                    sandbox.kill_active();
                                                    is_running = false;
                                                    if let Some(ss) = status_signals.as_ref() {
                                                        ss.send_stop();
                                                    }
                                                    agent_rx = None;
                                                }
                                            }
                                            None
                                        }
                                        Some(ask_req) = async {
                                            if let Some(rx) = ask_rx.as_mut() {
                                                rx.recv().await
                                            } else {
                                                std::future::pending().await
                                            }
                                        } => {
                                            let _ = handle_permission_request(
                                                ask_req, &mut renderer, session, cli,
                                                &mut user_rx, &mut agent_line_started,
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
                                let mcp_ref = ensure_mcp_manager(&mut mcp_manager, cfg).await;
                                handle_agent_event(
                                    ev,
                                    &mut renderer,
                                    session,
                                    cfg,
                                    cli,
                                    context,
                                    &mut is_running,
                                    &mut agent_rx,
                                    &mut agent_line_started,
                                    &mut merge_response_buf,
                                    &mut merge_response_start_line,
                                    &mut merge_was_reasoning,
                                    show_reasoning,
                                    &mut agent,
                                    &mut client,
                                    &mut loop_label,
                                    &permission,
                                    &ask_tx,
                                    &sandbox,
                                    &status_signals,
                                    #[cfg(feature = "loop")]
                                    &mut loop_state,
                                    #[cfg(feature = "git-worktree")]
                                    &mut wt_return_path,
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

#[cfg(feature = "advisor")]
async fn handle_human_handoff(
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
