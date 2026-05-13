use clap::Parser;
use compact_str::CompactString;

use crate::config;

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

    #[arg(long = "provider", env = "ZS_PROVIDER", help = "API provider")]
    pub provider: Option<String>,

    #[arg(long = "model", env = "ZS_MODEL", help = "Model name")]
    pub model: Option<String>,

    #[arg(long = "api-key", help = "API key for the provider")]
    pub api_key: Option<String>,

    #[arg(long = "max-tokens", help = "Maximum tokens in response")]
    pub max_tokens: Option<u64>,

    #[arg(long = "temperature", help = "Model temperature (0.0 to 2.0)")]
    pub temperature: Option<f64>,

    #[arg(short = 't', long = "tools", help = "Allowlist specific tools")]
    pub tools: Vec<String>,

    #[arg(long = "no-tools", help = "Disable all tools")]
    pub no_tools: bool,

    #[arg(
        long = "restrictive",
        short = 'R',
        help = "Default all tools to ask for approval"
    )]
    pub restrictive: bool,

    #[arg(
        long = "accept-all",
        help = "Auto-accept all operations within the working directory"
    )]
    pub accept_all: bool,

    #[arg(
        long = "yolo",
        help = "Auto-accept ALL operations without any restriction"
    )]
    pub yolo: bool,

    #[arg(
        long = "no-context-files",
        short = 'n',
        help = "Disable AGENTS.md loading"
    )]
    pub no_context_files: bool,

    #[arg(help = "Prompt message(s)")]
    pub message: Vec<String>,
}

impl Cli {
    pub fn resolve_model(&self, cfg: &config::Config) -> CompactString {
        self.model
            .as_deref()
            .or(cfg.model.as_deref())
            .map(CompactString::new)
            .unwrap_or_else(|| CompactString::new("deepseek/deepseek-v4-flash"))
    }

    pub fn resolve_provider(&self, cfg: &config::Config) -> CompactString {
        self.provider
            .as_deref()
            .or(cfg.provider.as_deref())
            .map(CompactString::new)
            .unwrap_or_else(|| CompactString::new("openrouter"))
    }

    pub fn resolve_max_tokens(&self, cfg: &config::Config) -> u64 {
        self.max_tokens.or(cfg.max_tokens).unwrap_or(8192)
    }

    pub fn resolve_no_context_files(&self, cfg: &config::Config) -> bool {
        self.no_context_files || cfg.no_context_files.unwrap_or(false)
    }

    pub fn resolve_no_tools(&self, cfg: &config::Config) -> bool {
        self.no_tools || cfg.no_tools.unwrap_or(false)
    }
}
