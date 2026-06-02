use compact_str::CompactString;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Token(CompactString),
    Reasoning(CompactString),
    ToolCall {
        name: CompactString,
        args: serde_json::Value,
    },
    ToolResult {
        name: CompactString,
        output: CompactString,
    },
    SubagentToolCall {
        name: CompactString,
        args: serde_json::Value,
    },
    Error(CompactString),
    Done {
        response: CompactString,
        input_tokens: u64,
        output_tokens: u64,
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
}
