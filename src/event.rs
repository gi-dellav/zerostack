use compact_str::CompactString;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Token(CompactString),
    Reasoning(CompactString),
    ToolCall {
        /// Rig's `internal_call_id` for this call: unique within the process
        /// (unlike the provider-supplied `ToolCall.id`, which Gemini/Ollama
        /// set to the function name). Providers may emit a whole batch of
        /// parallel calls before any of their results, so consumers must pair
        /// a `ToolResult` with its call by this id, never by "most recent
        /// call".
        call_id: CompactString,
        name: CompactString,
        args: serde_json::Value,
    },
    ToolResult {
        /// The [`AgentEvent::ToolCall::call_id`] this result answers.
        call_id: CompactString,
        name: CompactString,
        output: CompactString,
    },
    #[cfg(any(feature = "subagents", feature = "acp"))]
    SubagentToolCall {
        name: CompactString,
        args: serde_json::Value,
    },
    Error(CompactString),
    Retrying {
        attempt: usize,
        max: usize,
    },
    /// Provider call finished mid-stream. Carries the real provider-reported
    /// token usage for that call (when available). Used to update the
    /// status-bar estimate and to drive mid-turn compaction decisions
    /// independently of the local `len()/4` heuristic.
    CompletionCall {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
    Done {
        response: CompactString,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
}

/// Events emitted by an isolated `/btw` side-question run. Kept as a separate
/// type from [`AgentEvent`] so that a side-question result can never be routed
/// through `handle_agent_event` (which mutates the session): the type system
/// enforces that `/btw` leaves no trace in conversation history.
#[derive(Debug, Clone)]
pub enum BtwEvent {
    Done {
        id: u32,
        response: CompactString,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
    Error {
        id: u32,
        message: CompactString,
    },
}

#[derive(Debug, Clone)]
pub enum UserEvent {
    Key(crossterm::event::KeyEvent),
    ScrollUp,
    ScrollDown,
    Resize,
    Paste(String),
    #[allow(dead_code)]
    MouseDown {
        row: u16,
        col: u16,
    },
    #[allow(dead_code)]
    MouseDrag {
        row: u16,
        col: u16,
    },
    #[allow(dead_code)]
    MouseUp {
        row: u16,
        col: u16,
    },
    /// An interactive MCP OAuth login finished in a background task. `error` is
    /// `None` on success. Handled by the TUI loop to reconnect the server.
    #[cfg(feature = "mcp")]
    McpLoginDone {
        server: CompactString,
        error: Option<CompactString>,
    },
}
