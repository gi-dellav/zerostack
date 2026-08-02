//! Integration tests for the TUI main loop (`ui::app::App`), run headless:
//!
//! - **Fake `UserEvent` source**: no crossterm event thread; tests inject
//!   events straight into the loop's channel via `App::inject`.
//! - **Fake renderer backend**: `renderer::FakeBackend` pins a fixed 80x24
//!   geometry and captures every emitted frame, so nothing touches a real
//!   terminal (no raw mode, no alternate screen).
//! - **Scripted agent**: `AnyAgent::Mock` wraps rig's `MockCompletionModel`
//!   (see `fake_model.rs`), so a full user-prompt → agent-response round trip
//!   runs with no network.
//!
//! The loop is pumped one iteration at a time via `App::step`, which makes
//! assertions between event dispatch deterministic: on the current-thread
//! runtime used by `#[tokio::test]`, injected user events are always drained
//! before the freshly spawned agent task is first polled.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::UserEvent;
use crate::provider::AnyAgent;
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::tests::fake_model::{self, FakeModel};
use crate::ui::app::App;
use crate::ui::renderer::FakeBackend;
use crate::ui::state::UiContext;

/// Serializes every test in this file: they share process-global state
/// (`ZS_DATA_DIR`/`ZS_CONFIG_DIR` env, the statusline `OnceLock`, the subagent
/// event-sender singleton set by `runner::spawn_agent`, the model cache).
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn acquire() -> MutexGuard<'static, ()> {
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

fn isolate_data_dirs() {
    let dir = std::env::temp_dir().join(format!("zerostack-tui-loop-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { std::env::set_var("ZS_DATA_DIR", &dir) };
    unsafe { std::env::set_var("ZS_CONFIG_DIR", &dir) };
}

/// Build a headless `App` with a scripted mock agent. `turns` scripts one
/// model response per agent run (plain text chunks per turn). Returns the app
/// and the model (for inspecting the requests the loop sent).
async fn headless_app(turns: Vec<Vec<&str>>) -> (App<'static>, FakeModel) {
    isolate_data_dirs();
    let cli: &'static Cli = Box::leak(Box::new(Cli {
        api_key: Some("test-key".to_string()),
        no_session: true,
        no_color: true,
        ..Default::default()
    }));
    let cfg: &'static Config = Box::leak(Box::new(Config::default()));
    let session: &'static mut Session = Box::leak(Box::new(Session::new(
        "anthropic",
        "claude-sonnet-4-5",
        200_000,
        "tui-loop-test",
    )));
    let context: &'static mut ContextFiles = Box::leak(Box::new(crate::context::load(true)));
    let client =
        crate::provider::create_client("anthropic", Some("test-key"), &HashMap::new(), None)
            .expect("create test client");
    let ui = UiContext::new(
        cli,
        cfg,
        session,
        context,
        client,
        None,
        None,
        Sandbox::new(false, "bwrap"),
        None,
    );
    let model = fake_model::text_turns(turns);
    let agent = AnyAgent::Mock(rig::agent::AgentBuilder::new(model.clone()).build());
    let app = App::new_headless(
        ui,
        Some(agent),
        None,
        None,
        Box::new(FakeBackend::new(80, 24)),
    )
    .await
    .expect("build headless app");
    (app, model)
}

fn char_key(c: char) -> UserEvent {
    UserEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn enter_key() -> UserEvent {
    UserEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

/// Type `text` into the input editor and submit it with Enter.
async fn type_and_submit(app: &App<'static>, text: &str) {
    for c in text.chars() {
        app.inject(char_key(c)).await;
    }
    app.inject(enter_key()).await;
}

/// Pump one iteration, treating an idle (event-less) loop as a no-op instead
/// of blocking forever: `App::step`'s `select!` parks when no branch is ready,
/// which is correct in production (the event thread always feeds it) but would
/// hang a test whose condition can't be reached.
async fn pump(app: &mut App<'static>) {
    if let Ok(result) =
        tokio::time::timeout(std::time::Duration::from_millis(250), app.step()).await
    {
        let _ = result.expect("step failed");
    }
    // Err(_) = idle timeout: no events pending, nothing running.
}

/// Pump the loop until `done` holds, bounded so a stuck loop fails instead of
/// hanging the suite.
async fn step_until(app: &mut App<'static>, mut done: impl FnMut(&App<'static>) -> bool) {
    const MAX_STEPS: usize = 300;
    for _ in 0..MAX_STEPS {
        if done(app) {
            return;
        }
        pump(app).await;
    }
    assert!(done(app), "condition not met within {MAX_STEPS} steps");
}

#[tokio::test]
async fn submit_prompt_streams_response_and_updates_session() {
    let _guard = acquire();
    let (mut app, _model) = headless_app(vec![vec!["hi there"]]).await;

    type_and_submit(&app, "hello").await;
    step_until(&mut app, |a| a.is_running()).await;
    step_until(&mut app, |a| !a.is_running()).await;

    let messages = &app.session().messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content.as_str(), "hello");
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[1].content.as_str(), "hi there");

    let feed = app.feed_text();
    assert!(
        feed.contains("> hello"),
        "feed should echo the prompt: {feed}"
    );
    assert!(
        feed.contains("hi there"),
        "feed should show the response: {feed}"
    );

    app.teardown().await;
}

#[tokio::test]
async fn queued_input_replays_after_current_run() {
    let _guard = acquire();
    let (mut app, model) = headless_app(vec![vec!["answer one"], vec!["answer two"]]).await;

    // Inject everything up front: the injected events are drained before the
    // freshly spawned agent task is ever polled, so "second" is submitted
    // while the first run is still active and must be queued.
    type_and_submit(&app, "first").await;
    type_and_submit(&app, "second").await;

    step_until(&mut app, |a| a.feed_text().contains("queued: second")).await;
    step_until(&mut app, |a| {
        a.is_running() && a.session().messages.len() >= 3
    })
    .await;
    step_until(&mut app, |a| !a.is_running()).await;

    assert_eq!(
        model.requests().len(),
        2,
        "both turns should have reached the model"
    );
    let messages = &app.session().messages;
    let roles: Vec<(MessageRole, &str)> = messages
        .iter()
        .map(|m| (m.role, m.content.as_str()))
        .collect();
    assert_eq!(
        roles,
        vec![
            (MessageRole::User, "first"),
            (MessageRole::Assistant, "answer one"),
            (MessageRole::User, "second"),
            (MessageRole::Assistant, "answer two"),
        ]
    );

    app.teardown().await;
}

#[tokio::test]
async fn ctrl_c_exits_main_loop_when_idle() {
    let _guard = acquire();
    let (mut app, _model) = headless_app(vec![]).await;

    app.inject(UserEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )))
    .await;

    // The full `run()` returns cleanly once the loop breaks on Ctrl-C.
    app.run().await.expect("run should exit on Ctrl-C");
    app.teardown().await;
}

#[tokio::test]
async fn slash_clear_resets_session_and_feed() {
    let _guard = acquire();
    let (mut app, _model) = headless_app(vec![vec!["hi there"]]).await;

    type_and_submit(&app, "hello").await;
    step_until(&mut app, |a| {
        !a.is_running() && a.session().messages.len() == 2
    })
    .await;
    assert!(app.feed_text().contains("hi there"));

    // Typing "/" opens the command picker; the first Enter only completes the
    // highlighted command into the buffer ("/clear "), the second submits it —
    // same as the interactive flow.
    for c in "/clear".chars() {
        app.inject(char_key(c)).await;
    }
    app.inject(enter_key()).await;
    app.inject(enter_key()).await;
    step_until(&mut app, |a| {
        a.session().messages.is_empty() && !a.feed_text().contains("hi there")
    })
    .await;

    assert!(app.session().messages.is_empty());
    let feed = app.feed_text();
    assert!(
        feed.contains("zerostack"),
        "cleared feed should show the fresh welcome block: {feed}"
    );

    app.teardown().await;
}

#[tokio::test]
async fn fake_backend_captures_output_and_paste_fills_input() {
    let _guard = acquire();
    let (mut app, _model) = headless_app(vec![]).await;

    // The startup render already went through the fake backend.
    let captured = app.backend_output();
    assert!(
        captured.contains('\x1b'),
        "backend should capture ANSI frames, got {} bytes",
        captured.len()
    );

    app.inject(UserEvent::Paste("a\nb".to_string())).await;
    pump(&mut app).await;
    assert_eq!(app.input_buffer(), "a\nb");

    app.teardown().await;
}
