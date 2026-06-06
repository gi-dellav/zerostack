use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::event::AgentEvent;
use crate::provider::AnyClient;
use crate::session::SessionMessage;

pub(crate) mod builder;
pub(crate) mod prompt;
pub(crate) mod tool;

pub(crate) struct AdviserConfig {
    pub client: AnyClient,
    pub model_name: String,
    pub max_turns: usize,
}

static CONFIG: Mutex<Option<AdviserConfig>> = Mutex::new(None);

static ADVISER_EVENT_TX: Mutex<Option<mpsc::Sender<AgentEvent>>> = Mutex::new(None);

static SESSION_SNAPSHOT: Mutex<Vec<SessionMessage>> = Mutex::new(Vec::new());

pub(crate) fn set_adviser_event_tx(tx: mpsc::Sender<AgentEvent>) {
    let mut guard = ADVISER_EVENT_TX.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(tx);
}

pub(crate) fn clone_adviser_event_tx() -> Option<mpsc::Sender<AgentEvent>> {
    let guard = ADVISER_EVENT_TX.lock().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

pub fn update_session_snapshot(messages: &[SessionMessage]) {
    let mut guard = SESSION_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    guard.clear();
    guard.extend_from_slice(messages);
}

pub(crate) fn with_config<F, R>(f: F) -> R
where
    F: FnOnce(&AdviserConfig) -> R,
{
    let guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = guard
        .as_ref()
        .expect("adviser: AdviserConfig not initialized (call init() in main.rs)");
    f(cfg)
}

pub(crate) fn get_session_snapshot() -> Vec<SessionMessage> {
    let guard = SESSION_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

pub fn init(client: AnyClient, model_name: String, max_turns: usize) {
    let mut guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(AdviserConfig {
        client,
        model_name,
        max_turns,
    });
}
