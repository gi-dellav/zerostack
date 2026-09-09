//! Handler-level tests for the status signals that bracket the interactive
//! permission prompt.
//!
//! These drive `ui::permission_handler::handle_permission_request` directly:
//! a `Renderer` over `FakeBackend` (no terminal), a `UiContext` whose
//! `status_signals` point at a real `UnixListener` in a temp dir, and a
//! `user_rx` pre-loaded with the one key the prompt is meant to see. What
//! the listener collects is the wire truth for "the prompt reported its
//! wait", for every decision the prompt accepts and for the error path that
//! leaves the prompt without a decision at all.

#![cfg(unix)]

use std::collections::HashMap;
use std::future::Future;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::{mpsc, oneshot};

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::UserEvent;
use crate::extras::status_signals::StatusSignals;
use crate::permission::ask::{AskRequest, UserDecision};
use crate::sandbox::Sandbox;
use crate::session::Session;
use crate::ui::permission_handler::handle_permission_request;
use crate::ui::renderer::{FakeBackend, RenderBackend, Renderer};
use crate::ui::state::{AgentRunState, UiContext};

/// A listening status socket in a temp dir, plus the thread that drains it.
///
/// The senders connect once per message and write a single line, so the
/// accept loop turns the stream of connections into a flat list of lines in
/// arrival order.
struct SignalListener {
    dir: PathBuf,
    path: PathBuf,
    lines: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    accepter: Option<std::thread::JoinHandle<()>>,
}

impl SignalListener {
    fn bind(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("zs_sig_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        assert!(
            path.to_string_lossy().len() < 100,
            "socket path {:?} is too close to the 104-byte sun_path limit \
             (macOS CI runners fail bind() well before that with ENAMETOOLONG); \
             shorten the test name passed to SignalListener::bind",
            path
        );
        let listener = UnixListener::bind(&path).unwrap();
        // Non-blocking accept so the thread can notice the stop flag; the
        // accepted streams stay blocking, so each read runs to EOF.
        listener.set_nonblocking(true).unwrap();

        let lines = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let accepter = std::thread::spawn({
            let lines = Arc::clone(&lines);
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            let mut buf = String::new();
                            let _ = stream.read_to_string(&mut buf);
                            let mut collected = lines.lock().unwrap();
                            collected.extend(buf.lines().map(str::to_string));
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            }
        });

        Self {
            dir,
            path,
            lines,
            stop,
            accepter: Some(accepter),
        }
    }

    fn signals(&self) -> StatusSignals {
        StatusSignals::new(self.path.to_string_lossy().to_string())
    }

    /// Wait for `expected` lines, then keep waiting a beat so a surplus line
    /// would still show up: the assertions are about the exact sequence, not
    /// about a prefix of it.
    fn settle(&self, expected: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && self.lines.lock().unwrap().len() < expected {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(100));
        self.lines.lock().unwrap().clone()
    }
}

impl Drop for SignalListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.accepter.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A listening status socket with no accept thread behind it: every
/// connection the senders make stays queued in the kernel until the test
/// accepts it by hand.
///
/// `SignalListener` drains asynchronously, which is fine for "what was said"
/// but useless for "when was it said" - by the time its thread has the line,
/// the handler has moved on. Here the test decides the instant of
/// observation, so a line is visible exactly when the sender's `connect` +
/// `write` + close has already happened.
struct QueuedListener {
    dir: PathBuf,
    path: PathBuf,
    listener: UnixListener,
}

impl QueuedListener {
    fn bind(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("zs_sig_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        assert!(
            path.to_string_lossy().len() < 100,
            "socket path {:?} is too close to the 104-byte sun_path limit; \
             shorten the test name passed to QueuedListener::bind",
            path
        );
        let listener = UnixListener::bind(&path).unwrap();
        // Non-blocking accept so draining can stop at the end of the queue
        // instead of waiting for a connection that may never come.
        listener.set_nonblocking(true).unwrap();
        Self {
            dir,
            path,
            listener,
        }
    }

    fn signals(&self) -> StatusSignals {
        StatusSignals::new(self.path.to_string_lossy().to_string())
    }

    /// Take every connection that is queued *right now*, in arrival order,
    /// and flatten it into lines. Each sender writes one line and closes, so
    /// every accepted stream reads to EOF without blocking.
    fn drain(&self) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut buf = String::new();
                    stream.read_to_string(&mut buf).unwrap();
                    lines.extend(buf.lines().map(str::to_string));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("accept failed: {e}"),
            }
        }
        lines
    }
}

impl Drop for QueuedListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Waker for the tool side of the permission oneshot, which drains the status
/// socket the moment it is woken.
///
/// `oneshot::Sender::send` calls the registered waker inline, on the sending
/// thread, before it returns - so this snapshot is the state of the socket at
/// the exact instant the decision is handed to the waiting tool, not at some
/// later point the handler has already run past.
struct SnapshotOnWake {
    listener: Arc<QueuedListener>,
    at_send: Mutex<Option<Vec<String>>>,
}

impl Wake for SnapshotOnWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let lines = self.listener.drain();
        // Only the first wake is the send; a later one must not overwrite it.
        self.at_send.lock().unwrap().get_or_insert(lines);
    }
}

/// Render backend that fails every write once `marker` has appeared in the
/// bytes written so far. It turns one specific `renderer` call inside the
/// prompt loop into an `Err`, which is how the handler's `?` early return
/// gets exercised without touching production code.
struct FailOnMarkerBackend {
    buf: Vec<u8>,
    marker: &'static str,
    failed: bool,
}

impl FailOnMarkerBackend {
    fn new(marker: &'static str) -> Self {
        Self {
            buf: Vec::new(),
            marker,
            failed: false,
        }
    }
}

impl io::Write for FailOnMarkerBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        if self.failed || String::from_utf8_lossy(&self.buf).contains(self.marker) {
            self.failed = true;
            return Err(io::Error::other("backend write failed"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("backend flush failed"));
        }
        Ok(())
    }
}

impl RenderBackend for FailOnMarkerBackend {
    fn size(&self) -> io::Result<(u16, u16)> {
        Ok((80, 24))
    }
}

type PromptOutcome = (
    anyhow::Result<()>,
    Result<UserDecision, oneshot::error::TryRecvError>,
);

/// The `UiContext` every prompt test runs against: real config and session
/// objects, leaked so the context can outlive the borrows the handler takes.
fn leaked_ui(signals: Option<StatusSignals>) -> UiContext<'static> {
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
        "permission-signal-test",
    )));
    let context: &'static mut ContextFiles =
        Box::leak(Box::new(crate::context::load_with_prompts_dirs(true, &[])));
    let client =
        crate::provider::create_client("anthropic", Some("test-key"), &HashMap::new(), None)
            .expect("create test client");
    UiContext::new(
        cli,
        cfg,
        session,
        context,
        client,
        None,
        None,
        Sandbox::new(false, "bwrap"),
        signals,
    )
}

/// Run one permission prompt to completion: `key` is the single key the user
/// "presses", `signals` is what `UiContext` carries, `backend` is what the
/// renderer draws onto. Returns the handler's result and whatever the tool
/// side of the oneshot ended up with.
async fn run_prompt(
    key: KeyCode,
    signals: Option<StatusSignals>,
    backend: Box<dyn RenderBackend>,
) -> PromptOutcome {
    let mut ui = leaked_ui(signals);
    let mut renderer = Renderer::with_backend(backend);
    let mut run = AgentRunState::default();

    let (user_tx, mut user_rx) = mpsc::channel(4);
    user_tx
        .send(UserEvent::Key(KeyEvent::new(key, KeyModifiers::NONE)))
        .await
        .unwrap();

    let (reply_tx, mut reply_rx) = oneshot::channel();
    let ask_req = AskRequest {
        tool: "bash".into(),
        input: "ls -la".to_string(),
        reply: reply_tx,
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        handle_permission_request(ask_req, &mut renderer, &mut ui, &mut run, &mut user_rx),
    )
    .await
    .expect("handler did not return within 5s");
    let decision = reply_rx.try_recv();
    (result, decision)
}

fn fake_backend() -> Box<dyn RenderBackend> {
    Box::new(FakeBackend::new(80, 24))
}

const BRACKET: [&str; 2] = ["blocked:permission", "state:working"];

#[tokio::test]
async fn allow_once_brackets_wait() {
    let listener = SignalListener::bind("allow_once");
    let (result, decision) =
        run_prompt(KeyCode::Char('y'), Some(listener.signals()), fake_backend()).await;

    result.expect("prompt path succeeded");
    assert!(matches!(decision, Ok(UserDecision::AllowOnce)));
    assert_eq!(listener.settle(2), BRACKET);
}

#[tokio::test]
async fn allow_always_brackets_wait() {
    let listener = SignalListener::bind("allow_always");
    let (result, decision) =
        run_prompt(KeyCode::Char('a'), Some(listener.signals()), fake_backend()).await;

    result.expect("prompt path succeeded");
    assert!(matches!(decision, Ok(UserDecision::AllowAlways(_))));
    assert_eq!(listener.settle(2), BRACKET);
}

#[tokio::test]
async fn deny_brackets_wait() {
    let listener = SignalListener::bind("deny");
    let (result, decision) =
        run_prompt(KeyCode::Char('n'), Some(listener.signals()), fake_backend()).await;

    result.expect("prompt path succeeded");
    assert!(matches!(decision, Ok(UserDecision::Deny)));
    assert_eq!(listener.settle(2), BRACKET);
}

#[tokio::test]
async fn esc_brackets_wait() {
    let listener = SignalListener::bind("esc");
    let (result, decision) =
        run_prompt(KeyCode::Esc, Some(listener.signals()), fake_backend()).await;

    result.expect("prompt path succeeded");
    assert!(matches!(decision, Ok(UserDecision::Deny)));
    assert_eq!(listener.settle(2), BRACKET);
}

#[tokio::test]
async fn no_socket_emits_nothing() {
    let listener = SignalListener::bind("no_socket");
    let (result, decision) = run_prompt(KeyCode::Char('y'), None, fake_backend()).await;

    result.expect("prompt path succeeded");
    assert!(matches!(decision, Ok(UserDecision::AllowOnce)));
    assert!(listener.settle(0).is_empty());
}

/// The allow-always branch writes a confirmation line while the wait is still
/// open; a backend that fails on exactly that write makes the handler return
/// through `?` before any decision reaches the tool. The listener must still
/// see the wait end.
#[tokio::test]
async fn error_after_prompt_still_releases() {
    let listener = SignalListener::bind("error_path");
    let (result, decision) = run_prompt(
        KeyCode::Char('a'),
        Some(listener.signals()),
        Box::new(FailOnMarkerBackend::new("-> will allow")),
    )
    .await;

    assert!(result.is_err(), "expected the prompt path to fail");
    assert!(decision.is_err(), "no decision should reach the tool");
    assert_eq!(listener.settle(2), BRACKET);
}

/// The ordering half of the bracket: `state:working` must be on the wire
/// *before* the decision reaches the waiting tool, not merely by the time the
/// handler returns.
///
/// The two channels cannot be compared after the fact - the status socket is
/// drained by another thread, the decision by a oneshot - so the comparison
/// happens at the one instant both are pinned: the waker `oneshot::Sender::send`
/// invokes inline, before it returns. Registering our own waker on the tool
/// side of the oneshot and draining the socket from inside it makes the
/// snapshot a property of the kernel queue at send time. Moving the guard's
/// `drop` below `reply.send` in the handler leaves only `blocked:permission`
/// in that snapshot, which is the regression this pins.
#[tokio::test]
async fn state_working_reaches_the_wire_before_the_decision() {
    let listener = Arc::new(QueuedListener::bind("order"));
    let mut ui = leaked_ui(Some(listener.signals()));
    let mut renderer = Renderer::with_backend(fake_backend());
    let mut run = AgentRunState::default();

    let (user_tx, mut user_rx) = mpsc::channel(4);
    user_tx
        .send(UserEvent::Key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        )))
        .await
        .unwrap();

    let (reply_tx, reply_rx) = oneshot::channel();
    let ask_req = AskRequest {
        tool: "bash".into(),
        input: "ls -la".to_string(),
        reply: reply_tx,
    };

    // Register the snapshotting waker and leave it registered: nothing else
    // polls this receiver, so the next wake it sees is the handler's send.
    let observer = Arc::new(SnapshotOnWake {
        listener: Arc::clone(&listener),
        at_send: Mutex::new(None),
    });
    let waker = Waker::from(Arc::clone(&observer));
    let mut cx = Context::from_waker(&waker);
    let mut reply_rx = pin!(reply_rx);
    assert!(
        reply_rx.as_mut().poll(&mut cx).is_pending(),
        "no decision can exist before the prompt runs"
    );

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        handle_permission_request(ask_req, &mut renderer, &mut ui, &mut run, &mut user_rx),
    )
    .await
    .expect("handler did not return within 5s");
    result.expect("prompt path succeeded");

    let at_send = observer
        .at_send
        .lock()
        .unwrap()
        .clone()
        .expect("the handler never handed a decision back");
    assert_eq!(
        at_send, BRACKET,
        "state:working must already be on the socket when the decision reaches the tool"
    );
    assert!(
        matches!(
            reply_rx.as_mut().poll(&mut cx),
            Poll::Ready(Ok(UserDecision::AllowOnce))
        ),
        "the decision the prompt took"
    );
}
