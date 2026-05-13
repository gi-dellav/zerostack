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
        output: CompactString,
    },
    Error(CompactString),
    Done {
        response: CompactString,
        tokens: u64,
        cost: f64,
    },
}

#[derive(Debug, Clone)]
pub enum UserEvent {
    Key(crossterm::event::KeyEvent),
    ScrollUp,
    ScrollDown,
    #[allow(dead_code)]
    MouseDown { row: u16, col: u16 },
    #[allow(dead_code)]
    MouseDrag { row: u16, col: u16 },
    #[allow(dead_code)]
    MouseUp { row: u16, col: u16 },
}
