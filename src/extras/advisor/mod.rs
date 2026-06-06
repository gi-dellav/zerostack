use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::event::AgentEvent;
use crate::provider::AnyClient;
use crate::session::SessionMessage;

pub(crate) mod builder;
pub mod capabilities;
pub mod config;
pub(crate) mod events;
pub mod help;
pub mod init;
pub(crate) mod prompt;
pub mod slash;
pub(crate) mod tool;

pub(crate) struct AdvisorConfig {
    pub client: AnyClient,
    pub model_name: String,
    pub max_turns: usize,
}

static CONFIG: Mutex<Option<AdvisorConfig>> = Mutex::new(None);

static ADVISOR_EVENT_TX: Mutex<Option<mpsc::Sender<AgentEvent>>> = Mutex::new(None);

static SESSION_SNAPSHOT: Mutex<Vec<SessionMessage>> = Mutex::new(Vec::new());

pub(crate) fn set_advisor_event_tx(tx: mpsc::Sender<AgentEvent>) {
    let mut guard = ADVISOR_EVENT_TX.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(tx);
}

pub(crate) fn clone_advisor_event_tx() -> Option<mpsc::Sender<AgentEvent>> {
    let guard = ADVISOR_EVENT_TX.lock().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

pub fn update_session_snapshot(messages: &[SessionMessage]) {
    let mut guard = SESSION_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    guard.clear();
    guard.extend_from_slice(messages);
}

pub(crate) fn with_config<F, R>(f: F) -> R
where
    F: FnOnce(&AdvisorConfig) -> R,
{
    let guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = guard
        .as_ref()
        .expect("advisor: AdvisorConfig not initialized (call init() in main.rs)");
    f(cfg)
}

pub(crate) fn get_session_snapshot() -> Vec<SessionMessage> {
    let guard = SESSION_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

pub fn init(client: AnyClient, model_name: String, max_turns: usize) {
    let mut guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(AdvisorConfig {
        client,
        model_name,
        max_turns,
    });
}
