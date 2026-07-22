use clap::Parser;
use compact_str::CompactString;

use crate::config;
use crate::config::types::EditSystem;

#[derive(Parser, Debug, Default, Clone)]
#[command(name = "zerostack", version, about = "Minimal coding agent")]
pub struct Cli {
    #[arg(short = 'p', long = "print", help = "Print response and exit")]
    pub print: bool,

    #[arg(
        long = "pure-stdout",
        help = "With -p: also print tool calls/results to stdout"
    )]
    pub pure_stdout: bool,

    #[arg(long = "load-prompt", help = "Load a named prompt (same as /prompt)")]
    pub load_prompt: Option<String>,

    #[arg(long = "print-config", help = "Print resolved configuration and exit")]
    pub print_config: bool,

    #[arg(long = "setup", help = "Interactive setup wizard")]
    pub setup: bool,

    #[arg(long = "tutor", help = "Show getting started guide")]
    pub tutor: bool,

    #[arg(short = 'c', long = "continue", help = "Continue most recent session")]
    pub continue_session: bool,

    #[arg(short = 'r', long = "resume", help = "List recent sessions")]
    pub resume: bool,

    #[arg(long = "session", help = "Load session by ID prefix")]
    pub session: Option<String>,

    #[arg(long = "name", help = "Name for the session")]
    pub name: Option<String>,

    #[arg(long = "no-session", help = "Ephemeral mode, do not save")]
    pub no_session: bool,

    #[arg(long = "provider", env = "ZS_PROVIDER", help = "API provider")]
    pub provider: Option<String>,

    #[arg(long = "model", env = "ZS_MODEL", help = "Model name")]
    pub model: Option<String>,

    #[arg(long = "quick-model", help = "Use a named quick model from config")]
    pub quick_model: Option<String>,

    #[arg(
        long = "api-key",
        help = "API key for the provider (WARNING: visible to other users via ps/htop; prefer env vars)"
    )]
    pub api_key: Option<String>,

    #[arg(long = "max-tokens", help = "Maximum tokens in response")]
    pub max_tokens: Option<u64>,

    #[arg(long = "max-agent-turns", help = "Maximum agent turns")]
    pub max_agent_turns: Option<usize>,

    #[arg(long = "temperature", help = "Model temperature (0.0 to 2.0)")]
    pub temperature: Option<f64>,

    #[arg(
        short = 't',
        long = "tools",
        value_delimiter = ',',
        help = "Allowlist specific tools"
    )]
    pub tools: Vec<String>,

    #[arg(long = "no-tools", help = "Disable all tools")]
    pub no_tools: bool,

    #[arg(long = "no-color", help = "Disable colored TUI output")]
    pub no_color: bool,

    #[cfg(feature = "hooks")]
    #[arg(long = "no-hooks", help = "Disable all hooks")]
    pub no_hooks: bool,

    #[cfg(feature = "hooks")]
    #[arg(
        long = "hooks-test",
        value_name = "TOOL",
        help = "Dry-run PreToolUse hooks for TOOL with --hooks-test-input, print the merged decision, and exit"
    )]
    pub hooks_test: Option<String>,

    #[cfg(feature = "hooks")]
    #[arg(
        long = "hooks-test-input",
        help = "tool_input JSON for --hooks-test (default: {})"
    )]
    pub hooks_test_input: Option<String>,

    #[arg(long = "restrictive", short = 'R', help = "Ask for all operations")]
    pub restrictive: bool,

    #[arg(long = "read-only", help = "Allow reads only, deny everything else")]
    pub read_only: bool,

    #[arg(long = "guarded", help = "Allow reads, ask for all other operations")]
    pub guarded: bool,

    #[arg(
        long = "accept-all",
        help = "Auto-accept all operations within the working directory"
    )]
    pub accept_all: bool,

    #[arg(
        long = "yolo",
        help = "Allow all operations except destructive bash commands"
    )]
    pub yolo: bool,

    #[arg(
        long = "dangerously-skip-permissions",
        help = "Skip all permission checks (allow everything without any guard)"
    )]
    pub dangerously_skip_permissions: bool,

    #[arg(
        long = "sandbox",
        help = "Run bash commands inside bubblewrap (bwrap) sandbox"
    )]
    pub sandbox: bool,

    #[arg(
        long = "sandbox-backend",
        help = "Sandbox backend: bwrap (default) or zerobox"
    )]
    pub sandbox_backend: Option<String>,

    #[arg(
        long = "shell",
        help = "Shell binary to use for bash tool (default: bash)"
    )]
    pub shell: Option<String>,

    #[arg(
        long = "edit-system",
        help = "Edit system (similarity or hashedit). Default: similarity"
    )]
    pub edit_system: Option<String>,

    #[arg(
        long = "no-context-files",
        short = 'n',
        help = "Disable AGENTS.md and ARCHITECTURE.md loading"
    )]
    pub no_context_files: bool,

    #[cfg(feature = "loop")]
    #[arg(
        long = "loop",
        help = "Run in headless loop mode (requires --loop-prompt or message)"
    )]
    pub loop_mode: bool,

    #[cfg(feature = "acp")]
    #[arg(
        long = "acp",
        help = "Enable ACP (Agent Communication Protocol) support"
    )]
    pub acp_enabled: bool,

    #[cfg(feature = "acp")]
    #[arg(long = "acp-host", help = "ACP TCP bind host [default: stdio mode]")]
    pub acp_host: Option<String>,

    #[cfg(feature = "acp")]
    #[arg(long = "acp-port", help = "ACP TCP bind port [default: 7243]")]
    pub acp_port: Option<u16>,

    #[cfg(feature = "loop")]
    #[arg(long = "loop-prompt", help = "Prompt for each loop iteration")]
    pub loop_prompt: Option<String>,

    #[cfg(feature = "loop")]
    #[arg(long = "loop-plan", help = "Plan file path [default: LOOP_PLAN.md]")]
    pub loop_plan: Option<std::path::PathBuf>,

    #[cfg(feature = "loop")]
    #[arg(long = "loop-max", help = "Maximum number of iterations")]
    pub loop_max: Option<u32>,

    #[cfg(feature = "loop")]
    #[arg(
        long = "loop-run",
        help = "Validation command to run after each iteration"
    )]
    pub loop_run: Option<String>,

    #[cfg(feature = "git-worktree")]
    #[arg(long = "worktree", help = "Create a git worktree and cd into it")]
    pub worktree: Option<String>,

    #[cfg(feature = "git-worktree")]
    #[arg(long = "wt-auto-merge", help = "Auto-merge worktree branch on exit")]
    pub wt_auto_merge: bool,

    #[cfg(feature = "git-worktree")]
    #[arg(
        long = "parallel",
        help = "Create a worktree with timestamp name and auto-merge on exit"
    )]
    pub parallel: bool,

    #[cfg(feature = "git-worktree")]
    #[arg(
        long = "wt-base-dir",
        help = "Base directory for worktrees (default: parent of current repo)"
    )]
    pub wt_base_dir: Option<String>,

    #[cfg(feature = "git-worktree")]
    #[arg(
        long = "wt-force",
        help = "Force worktree remove and branch delete even if dirty"
    )]
    pub wt_force: bool,

    #[cfg(feature = "advisor")]
    #[arg(
        long = "advisor",
        help = "Enable advisor tool (model can consult a stronger reviewer model)"
    )]
    pub advisor: bool,

    #[cfg(feature = "advisor")]
    #[arg(
        long = "advisor-model",
        help = "Advisor model name (e.g. 'claude-opus-4-8')"
    )]
    pub advisor_model: Option<String>,

    #[cfg(feature = "advisor")]
    #[arg(
        long = "advisor-max-uses",
        help = "Maximum advisor calls per request (default: 3)"
    )]
    pub advisor_max_uses: Option<usize>,

    #[cfg(feature = "advisor")]
    #[arg(
        long = "advisor-human-handoff",
        help = "Route advisor calls to the user instead of a model",
        default_missing_value = "true",
        num_args = 0..=1,
        require_equals = true,
        default_value = "false"
    )]
    pub advisor_human_handoff: Option<bool>,

    #[cfg(feature = "advisor")]
    #[arg(
        long = "advisor-kilobytes-limit",
        help = "Max total kilobytes of conversation context to send to the advisor (head: half, tail: half). Default: 256",
        default_value = "256"
    )]
    pub advisor_kilobytes_limit: u32,

    #[cfg(feature = "status-signals")]
    #[arg(
        long = "status-socket",
        help = "Unix socket path for status signals (start/stop messages)"
    )]
    pub status_socket: Option<String>,

    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::SetTrue,
          help = "Enable full logging (trace level) to a timestamped log file under the data directory")]
    pub verbose: bool,

    #[arg(
        long = "log-file",
        help = "Write logs to this file (overrides verbose default path)"
    )]
    pub log_file: Option<std::path::PathBuf>,

    #[arg(
        long = "log-level",
        help = "Set stderr log level (trace, debug, info, warn, error)"
    )]
    pub log_level: Option<String>,

    #[arg(help = "Prompt message(s)")]
    pub message: Vec<String>,
}

impl Cli {
    pub fn resolve_quick_model<'a>(
        &self,
        cfg: &'a config::Config,
    ) -> Option<&'a config::QuickModelConfig> {
        let name = self.quick_model.as_deref()?;
        cfg.quick_models.as_ref().and_then(|m| m.get(name))
    }

    pub fn resolve_model(&self, cfg: &config::Config) -> CompactString {
        // CLI --model takes a raw model string.
        if let Some(m) = self.model.as_deref() {
            return CompactString::new(m);
        }
        // Config model field references a quick model name; resolve it.
        if let Some(m) = cfg.model.as_deref() {
            let qm = config::quick_models_map(cfg);
            if let Some(q) = qm.get(m) {
                return q.model.clone();
            }
            return CompactString::new(m);
        }
        // No explicit model. If a provider was chosen explicitly, default to a
        // model valid for it so `--provider anthropic` does not keep the
        // OpenRouter default id; otherwise keep the historic deepseek default.
        if (self.provider.is_some() || cfg.provider.is_some())
            && let Some((model, _)) =
                crate::provider::default_model_for_provider(&self.resolve_provider(cfg), cfg)
        {
            return CompactString::new(model);
        }
        let qm = config::quick_models_map(cfg);
        qm.get("deepseek-v4-pro")
            .map(|q| q.model.clone())
            .unwrap_or_else(|| CompactString::new("deepseek/deepseek-v4-pro"))
    }

    pub fn resolve_provider(&self, cfg: &config::Config) -> CompactString {
        self.provider
            .as_deref()
            .or(cfg.provider.as_deref())
            .map(CompactString::new)
            .unwrap_or_else(|| {
                let qm = config::quick_models_map(cfg);
                qm.get("deepseek-v4-pro")
                    .map(|q| q.provider.clone())
                    .unwrap_or_else(|| CompactString::new("openrouter"))
            })
    }

    pub fn resolve_max_tokens(&self, cfg: &config::Config) -> u64 {
        self.max_tokens.or(cfg.max_tokens).unwrap_or(16384)
    }

    pub fn resolve_max_agent_turns(&self, cfg: &config::Config) -> usize {
        self.max_agent_turns.or(cfg.max_agent_turns).unwrap_or(200)
    }

    pub fn resolve_no_context_files(&self, cfg: &config::Config) -> bool {
        self.no_context_files || cfg.no_context_files.unwrap_or(false)
    }

    pub fn resolve_no_tools(&self, cfg: &config::Config) -> bool {
        self.no_tools || cfg.no_tools.unwrap_or(false)
    }

    pub fn resolve_sandbox(&self, cfg: &config::Config) -> bool {
        self.sandbox || cfg.sandbox.unwrap_or(false)
    }

    pub fn resolve_sandbox_backend(&self, cfg: &config::Config) -> String {
        self.sandbox_backend
            .clone()
            .or_else(|| cfg.sandbox_backend.clone())
            .unwrap_or_else(|| "bwrap".to_string())
    }

    pub fn resolve_shell(&self, cfg: &config::Config) -> String {
        self.shell
            .clone()
            .or_else(|| cfg.shell.clone())
            .unwrap_or_else(|| "bash".to_string())
    }

    pub fn resolve_edit_system(&self, cfg: &config::Config) -> EditSystem {
        self.edit_system
            .as_deref()
            .and_then(|s| s.parse().ok())
            .or(cfg.edit_system)
            .unwrap_or_default()
    }

    #[cfg(feature = "git-worktree")]
    pub fn resolve_wt_auto_merge(&self, cfg: &config::Config) -> bool {
        self.wt_auto_merge || self.parallel || cfg.wt_auto_merge.unwrap_or(false)
    }

    #[cfg(feature = "git-worktree")]
    pub fn resolve_wt_base_dir(&self, cfg: &config::Config) -> Option<std::path::PathBuf> {
        self.wt_base_dir
            .clone()
            .or_else(|| cfg.wt_base_dir.clone())
            .map(std::path::PathBuf::from)
    }

    #[cfg(feature = "git-worktree")]
    pub fn resolve_wt_force(&self, cfg: &config::Config) -> bool {
        self.wt_force || cfg.wt_force.unwrap_or(false)
    }

    #[cfg(feature = "advisor")]
    pub fn resolve_advisor_enabled(&self, cfg: &config::Config) -> bool {
        if let Some(ref ac) = cfg.advisor {
            self.advisor || ac.enabled
        } else {
            self.advisor
        }
    }

    #[cfg(feature = "advisor")]
    pub fn resolve_advisor_model(&self, cfg: &config::Config) -> String {
        self.advisor_model
            .clone()
            .or_else(|| {
                cfg.advisor
                    .as_ref()
                    .and_then(|a| a.model.clone())
                    .map(|m| m.to_string())
            })
            .unwrap_or_else(|| "deepseek-v4-pro".to_string())
    }

    #[cfg(feature = "advisor")]
    pub fn resolve_advisor_max_uses(&self, cfg: &config::Config) -> Option<usize> {
        self.advisor_max_uses
            .or_else(|| cfg.advisor.as_ref().and_then(|a| a.max_uses))
    }

    #[cfg(feature = "advisor")]
    pub fn resolve_advisor_human_handoff(&self, cfg: &config::Config) -> bool {
        self.advisor_human_handoff.unwrap_or_else(|| {
            cfg.advisor
                .as_ref()
                .map(|a| a.human_handoff)
                .unwrap_or(false)
        })
    }

    #[cfg(feature = "advisor")]
    pub fn resolve_advisor_kilobytes_limit(&self, cfg: &config::Config) -> u32 {
        if self.advisor_kilobytes_limit != 256 {
            self.advisor_kilobytes_limit
        } else {
            cfg.advisor
                .as_ref()
                .map(|a| a.advisor_kilobytes_limit)
                .unwrap_or(256)
        }
    }
}
