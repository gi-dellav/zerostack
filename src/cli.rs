use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "zerostack", version, about = "Minimal coding agent")]
pub struct Cli {
    #[arg(short = 'p', long = "print", help = "Print response and exit")]
    pub print: bool,

    #[arg(short = 'c', long = "continue", help = "Continue most recent session")]
    pub continue_session: bool,

    #[arg(short = 'r', long = "resume", help = "Browse and select a session")]
    pub resume: bool,

    #[arg(long = "session", help = "Use specific session file or ID")]
    pub session: Option<String>,

    #[arg(long = "no-session", help = "Ephemeral mode, do not save")]
    pub no_session: bool,

    #[arg(long = "provider", env = "ZS_PROVIDER", default_value = "openrouter")]
    pub provider: String,

    #[arg(
        long = "model",
        env = "ZS_MODEL",
        default_value = "deepseek/deepseek-v4-flash"
    )]
    pub model: String,

    #[arg(
        long = "api-key",
        env = "ZS_API_KEY",
        help = "API key for the provider"
    )]
    pub api_key: Option<String>,

    #[arg(long = "max-tokens", default_value = "8192")]
    pub max_tokens: u64,

    #[arg(long = "temperature")]
    pub temperature: Option<f64>,

    #[arg(short = 't', long = "tools", help = "Allowlist specific tools")]
    pub tools: Vec<String>,

    #[arg(long = "no-tools", help = "Disable all tools")]
    pub no_tools: bool,

    #[arg(long = "no-context-files", short = 'n', help = "Disable AGENTS.md loading")]
    pub no_context_files: bool,

    #[arg(help = "Prompt message(s)")]
    pub message: Vec<String>,
}
