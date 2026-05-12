#[derive(Debug, Clone)]
pub enum AgentEvent {
    Token(String),
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        output: String,
    },
    Error(String),
    Done {
        response: String,
    },
}

#[derive(Debug, Clone)]
pub enum UserEvent {
    Key(crossterm::event::KeyEvent),
    Quit,
}
