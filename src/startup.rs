use compact_str::CompactString;

use crate::agent::tools;
use crate::cli::Cli;
use crate::config::{self, Config};
use crate::context::{self, ContextFiles};
use crate::extras::status_signals::StatusSignals;
use crate::permission::SecurityMode;
use crate::permission::ask::{AskReceiver, AskSender};
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::provider::{self, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{self, MessageRole, Session};

#[cfg(feature = "advisor")]
use crate::session::SessionMessage;

// ── Helper functions ─────────────────────────────────────────────────────

/// Regenerate embedded prompts/themes, printing the actual outcome instead
/// of claiming success on failure. Returns true when regeneration succeeded.
fn regen_resource(regen: fn() -> anyhow::Result<()>, what: &str, suffix: &str) -> bool {
    match regen() {
        Ok(()) => {
            eprintln!("{} regenerated{}.", what, suffix);
            true
        }
        Err(e) => {
            eprintln!(
                "warning: failed to regenerate {}: {}",
                what.to_lowercase(),
                e
            );
            false
        }
    }
}

fn resolve_mode(cli: &Cli, cfg: &Config) -> SecurityMode {
    if cli.yolo || cfg.yolo.unwrap_or(false) {
        SecurityMode::Yolo
    } else if cli.accept_all || cfg.accept_all.unwrap_or(false) {
        SecurityMode::Standard
    } else if cli.read_only {
        SecurityMode::ReadOnly
    } else if cli.guarded {
        SecurityMode::Guarded
    } else if cli.restrictive || cfg.restrictive.unwrap_or(false) {
        SecurityMode::Restrictive
    } else if let Some(m) = &cfg.default_permission_mode {
        match m.as_str() {
            "yolo" => SecurityMode::Yolo,
            "accept" => SecurityMode::Standard,
            "standard" => SecurityMode::Standard,
            "guarded" => SecurityMode::Guarded,
            "readonly" => SecurityMode::ReadOnly,
            "restrictive" => SecurityMode::Restrictive,
            _ => SecurityMode::Standard,
        }
    } else {
        SecurityMode::Standard
    }
}

fn build_permission_checker(
    cli: &Cli,
    cfg: &Config,
) -> (Option<PermCheck>, Option<AskSender>, Option<AskReceiver>) {
    let no_tools = cli.resolve_no_tools(cfg);
    if no_tools {
        return (None, None, None);
    }

    if cli.dangerously_skip_permissions {
        return (None, None, None);
    }

    let perm_config = cfg.build_permission_config();

    let mode = resolve_mode(cli, cfg);
    let permission_modes = cfg.permission_modes.clone();
    let checker = PermissionChecker::new(&perm_config, mode, None, permission_modes);
    let perm: PermCheck = std::sync::Arc::new(std::sync::Mutex::new(checker));

    let (ask_tx, ask_rx) = tokio::sync::mpsc::channel(64);
    (Some(perm), Some(ask_tx), Some(ask_rx))
}

/// Apply the `[prompt_to_model]` mapping at startup before the TUI is
/// available. Updates `provider`, `model`, and `session` fields so the
/// initial agent is built with the correct model.
fn apply_startup_prompt_model(
    prompt_name: &str,
    cfg: &Config,
    provider: &mut CompactString,
    model: &mut CompactString,
    session: &mut Session,
) {
    let qm_name = match cfg.resolve_prompt_model(prompt_name) {
        Some(name) => name,
        None => return,
    };
    let qm = config::quick_models_map(cfg);
    let Some(qmc) = qm.get(qm_name) else {
        return;
    };
    *provider = qmc.provider.clone();
    *model = qmc.model.clone();
    session.model = qmc.model.clone();
    session.provider = qmc.provider.clone();
    session.input_token_cost = qmc.input_token_cost;
    session.output_token_cost = qmc.output_token_cost;
    session.update_context_window(cfg.resolve_context_window(
        &session.provider,
        &session.model,
        &qm,
    ));
}

/// Connect configured MCP servers for a headless (`-p`/`--loop`) run. Unlike
/// the TUI (`ui::ensure_mcp_manager`), headless has no alt-screen to protect,
/// so connection failures are printed to stderr instead of staying silent
/// until surfaced by the renderer.
#[cfg(feature = "mcp")]
pub(crate) async fn connect_headless_mcp(
    cfg: &Config,
) -> Option<crate::extras::mcp::McpClientManager> {
    let servers = cfg.mcp_servers.as_ref()?;
    if servers.is_empty() {
        return None;
    }
    let manager = crate::extras::mcp::McpClientManager::connect_all(servers).await;
    for notice in &manager.notices {
        eprintln!("{}", notice);
    }
    Some(manager)
}

// ── Startup state ────────────────────────────────────────────────────────

pub(crate) struct Startup {
    pub cli: Cli,
    pub cfg: Config,
    pub is_first_startup: bool,
    pub context: ContextFiles,
    pub provider: CompactString,
    pub model: CompactString,
    pub session: Session,
    pub client: AnyClient,
    pub is_interactive: bool,
    pub version_changed: bool,
    // Set by init_features:
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub ask_rx: Option<AskReceiver>,
    pub sandbox: Sandbox,
    pub status_signals: Option<StatusSignals>,
    #[cfg(feature = "advisor")]
    pub handoff_rx: Option<crate::extras::advisor::HandoffReceiver>,
    // Set by resolve_prompts:
    pub arch_msg: Option<String>,
    #[cfg(feature = "hooks")]
    pub session_resumed: bool,
}

impl Startup {
    /// Phase 1: context load, provider/model resolution, session
    /// creation/resolution, client creation.
    pub(crate) async fn init(
        cli: Cli,
        cfg: Config,
        is_first_startup: bool,
        version_changed: bool,
        is_interactive: bool,
    ) -> anyhow::Result<Self> {
        // Load context first so prompts/themes are available early.
        let context = context::load(cli.resolve_no_context_files(&cfg));

        let mut provider = cli.resolve_provider(&cfg);
        let mut model = cli.resolve_model(&cfg);

        // --quick-model overrides provider + model
        if let Some(qm) = cli.resolve_quick_model(&cfg) {
            provider = qm.provider.clone();
            model = qm.model.clone();
        }

        let name = cli.name.as_deref().unwrap_or("");
        let qm_map = config::quick_models_map(&cfg);
        let mut session = Session::new(
            &provider,
            &model,
            cfg.resolve_context_window(&provider, &model, &qm_map),
            name,
        );

        // Resolve input/output token costs from quick models or defaults
        if let Some(qm) = cli.resolve_quick_model(&cfg) {
            session.input_token_cost = qm.input_token_cost;
            session.output_token_cost = qm.output_token_cost;
        } else if let Some(qm) = qm_map
            .iter()
            .find(|(_, v)| v.model.as_str() == model && v.provider.as_str() == provider)
            .map(|(_, v)| v)
        {
            session.input_token_cost = qm.input_token_cost;
            session.output_token_cost = qm.output_token_cost;
        } else if let Some((input_cost, output_cost)) =
            Config::catalog_input_output_cost(&provider, &model)
        {
            session.input_token_cost = input_cost;
            session.output_token_cost = output_cost;
        }

        #[cfg(feature = "hooks")]
        let mut session_resumed = false;

        if cli.continue_session
            && cli.session.is_none()
            && let Ok(sessions) = session::storage::find_recent_sessions(1)
            && let Some(s) = sessions.into_iter().next()
        {
            session = s;
            #[cfg(feature = "hooks")]
            {
                session_resumed = true;
            }
        }

        if let Some(session_id) = &cli.session {
            let sessions = session::storage::find_sessions_by_prefix(session_id)?;
            if sessions.is_empty() {
                // try exact name match as fallback
                if let Some(s) = session::storage::find_session_by_name(session_id)? {
                    session = s;
                    #[cfg(feature = "hooks")]
                    {
                        session_resumed = true;
                    }
                } else {
                    anyhow::bail!("no session matching '{}'", session_id);
                }
            } else if sessions.len() == 1 {
                session = sessions.into_iter().next().unwrap();
                #[cfg(feature = "hooks")]
                {
                    session_resumed = true;
                }
            } else {
                eprintln!("multiple sessions match '{}':", session_id);
                for s in &sessions {
                    let preview = s
                        .messages
                        .last()
                        .map(|m| {
                            let truncated: String = m.content.chars().take(40).collect();
                            truncated
                        })
                        .unwrap_or_default();
                    let time = crate::ui::events::format_time(&s.updated_at);
                    let name_part = if s.name.is_empty() {
                        String::new()
                    } else {
                        format!("  [{}]", s.name)
                    };
                    eprintln!(
                        "  {}  {}  {}msgs  {}  {}{}",
                        &s.id[..8],
                        time,
                        s.messages.len(),
                        s.model,
                        preview,
                        name_part
                    );
                }
                anyhow::bail!("be more specific with the session ID prefix");
            }
        }

        // A resumed session persisted its context_window when first saved, which can
        // be stale if the model's catalog entry has changed since (e.g. a model that
        // grew from 128k to 1M). Re-derive it from the catalog for the session's own
        // model, unless the user pinned `context_window` in config (then that wins).
        if cfg.context_window.is_none()
            && let Some(cw) =
                Config::catalog_context_window(session.provider.as_str(), session.model.as_str())
        {
            session.update_context_window(cw);
        }

        let client = provider::create_client(
            &provider,
            cli.api_key.as_deref(),
            &cfg.custom_providers_map(),
            cfg.api_keys.as_ref(),
        )?;

        Ok(Self {
            cli,
            cfg,
            is_first_startup,
            context,
            provider,
            model,
            session,
            client,
            is_interactive,
            version_changed,
            permission: None,
            ask_tx: None,
            ask_rx: None,
            sandbox: Sandbox::new(false, "bwrap"),
            status_signals: None,
            #[cfg(feature = "advisor")]
            handoff_rx: None,
            arch_msg: None,
            #[cfg(feature = "hooks")]
            session_resumed,
        })
    }

    /// Phase 2: subagents, OpenRouter pricing, sandbox, tools config,
    /// permission checker, advisor.
    pub(crate) async fn init_features(&mut self) -> anyhow::Result<()> {
        #[cfg(feature = "subagents")]
        {
            let task_max_turns = self.cfg.task_max_turns.unwrap_or(20);
            let qm = config::quick_models_map(&self.cfg);

            // Resolve subagent model: subagent_model config > subagent_provider + model > main model
            let (sub_provider, mut sub_model) = if let Some(sa_model) = &self.cfg.subagent_model {
                if let Some(q) = qm.get(sa_model.as_str()) {
                    (q.provider.clone(), q.model.clone())
                } else {
                    let prov = self
                        .cfg
                        .subagent_provider
                        .clone()
                        .unwrap_or_else(|| self.provider.clone());
                    (prov, sa_model.clone())
                }
            } else if let Some(sa_prov) = &self.cfg.subagent_provider {
                (sa_prov.clone(), self.model.clone())
            } else {
                (self.provider.clone(), self.model.clone())
            };

            let sub_client = if sub_provider.as_str() == self.provider.as_str() {
                self.client.clone()
            } else {
                match crate::provider::create_client(
                    &sub_provider,
                    self.cli.api_key.as_deref(),
                    &self.cfg.custom_providers_map(),
                    self.cfg.api_keys.as_ref(),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            "Could not initialize subagent provider '{}' ({}); \
                             falling back to main provider '{}'. \
                             Set `subagent_provider`/`subagent_model` in config, or the \
                             provider's API key, to silence this.",
                            sub_provider,
                            e,
                            self.provider
                        );
                        sub_model = self.model.clone();
                        self.client.clone()
                    }
                }
            };

            crate::extras::subagents::init(
                sub_client,
                sub_model.to_string(),
                task_max_turns,
                self.cfg.clone(),
                #[cfg(feature = "archmd")]
                self.context.architecture.clone(),
            );
        }

        // Fetch OpenRouter pricing and context window at startup so cost tracking
        // and the context meter work from the first turn.
        if self.provider == "openrouter" {
            let need_pricing =
                self.session.input_token_cost == 0.0 && self.session.output_token_cost == 0.0;
            let need_ctx = self.cfg.context_window.is_none()
                && Config::catalog_context_window("openrouter", self.model.as_str()).is_none();
            if (need_pricing || need_ctx)
                && let Ok(infos) = provider::fetch_openrouter_pricing(
                    self.cli.api_key.as_deref(),
                    &self.cfg.custom_providers_map(),
                    self.cfg.api_keys.as_ref(),
                )
                .await
                && let Some(info) = infos.get(self.model.as_str())
            {
                if need_pricing {
                    self.session.input_token_cost = info.input_cost;
                    self.session.output_token_cost = info.output_cost;
                }
                if need_ctx && let Some(cw) = info.context_length {
                    self.session.update_context_window(cw);
                }
            }
        }

        // Sandbox, tools config, status signals, permission checker
        self.sandbox = Sandbox::new(
            self.cli.resolve_sandbox(&self.cfg),
            &self.cli.resolve_sandbox_backend(&self.cfg),
        )
        .with_shell(&self.cli.resolve_shell(&self.cfg));
        if self.cli.resolve_sandbox(&self.cfg) && !self.sandbox.is_effectively_sandboxed() {
            tracing::warn!(
                "sandbox is enabled but backend '{}' was not found — commands will run unsandboxed",
                self.cli.resolve_sandbox_backend(&self.cfg)
            );
        }
        let edit_system = self.cli.resolve_edit_system(&self.cfg);
        tools::set_edit_system(edit_system);
        tools::set_deny_repeated_reads(self.cfg.deny_repeated_reads.unwrap_or(true));

        #[cfg(feature = "status-signals")]
        {
            self.status_signals = self.cli.status_socket.clone().map(StatusSignals::new);
        }

        let (permission, ask_tx, ask_rx) = build_permission_checker(&self.cli, &self.cfg);
        self.permission = permission;
        self.ask_tx = ask_tx;
        self.ask_rx = ask_rx;

        // Advisor setup
        #[cfg(feature = "advisor")]
        {
            let enabled = self.cli.resolve_advisor_enabled(&self.cfg);
            let human_handoff = self.cli.resolve_advisor_human_handoff(&self.cfg);
            let advisor_model_name = self.cli.resolve_advisor_model(&self.cfg);
            let max_uses = self.cli.resolve_advisor_max_uses(&self.cfg);
            let kilobytes_limit = self.cli.resolve_advisor_kilobytes_limit(&self.cfg);

            let qm = config::quick_models_map(&self.cfg);
            let (advisor_provider, advisor_model) =
                if let Some(q) = qm.get(advisor_model_name.as_str()) {
                    (q.provider.to_string(), q.model.to_string())
                } else {
                    (self.provider.to_string(), advisor_model_name)
                };

            let advisor_client = if advisor_provider == self.provider.as_str() {
                Some(self.client.clone())
            } else {
                match crate::provider::create_client(
                    &advisor_provider,
                    self.cli.api_key.as_deref(),
                    &self.cfg.custom_providers_map(),
                    self.cfg.api_keys.as_ref(),
                ) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        tracing::warn!(
                            "Could not create advisor client for provider '{}' ({}); \
                             advisor disabled. Set `advisor.model` and API key in config.",
                            advisor_provider,
                            e
                        );
                        None
                    }
                }
            };

            let (handoff_tx, handoff_rx) = if human_handoff && self.is_interactive {
                let (tx, rx) = tokio::sync::mpsc::channel(8);
                (Some(tx), Some(rx))
            } else {
                (None, None)
            };

            let config = crate::extras::advisor::AdvisorToolConfig {
                client: advisor_client,
                advisor_model,
                human_handoff,
                max_uses,
                handoff_tx,
                enabled,
                kilobytes_limit,
            };
            crate::extras::advisor::init_config(config);

            self.handoff_rx = handoff_rx;
        }

        Ok(())
    }

    /// Phase 3: version-change prompts, MCP recommendations, ARCHITECTURE.md,
    /// default prompt resolution, --load-prompt override, permission mode from
    /// prompt directive.
    pub(crate) async fn resolve_prompts(&mut self) -> anyhow::Result<()> {
        // Version-change prompts
        if self.version_changed && self.is_interactive && !self.is_first_startup {
            let prompts_dir = context::prompts::global_dir();
            let themes_dir = context::themes::global_dir();
            let mut regenerated = false;

            match self.cfg.resolve_auto_update_prompts() {
                Some(true) => {
                    regenerated |= regen_resource(context::prompts::regen, "Prompts", "");
                }
                Some(false) => { /* skip: user explicitly denied */ }
                None => {
                    if !prompts_dir.exists() {
                        regenerated |=
                            regen_resource(context::prompts::regen, "Prompts", " (first launch)");
                    } else {
                        let mut input = String::new();
                        eprint!("Regenerate prompts? [y/N] ");
                        let _ = std::io::Write::flush(&mut std::io::stderr());
                        std::io::stdin().read_line(&mut input)?;
                        if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                            regenerated |= regen_resource(context::prompts::regen, "Prompts", "");
                        }
                    }
                }
            }

            match self.cfg.resolve_auto_update_themes() {
                Some(true) => {
                    regenerated |= regen_resource(context::themes::regen, "Themes", "");
                }
                Some(false) => { /* skip: user explicitly denied */ }
                None => {
                    if !themes_dir.exists() {
                        regenerated |=
                            regen_resource(context::themes::regen, "Themes", " (first launch)");
                    } else {
                        let mut input = String::new();
                        eprint!("Regenerate themes? [y/N] ");
                        let _ = std::io::Write::flush(&mut std::io::stderr());
                        std::io::stdin().read_line(&mut input)?;
                        if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                            regenerated |= regen_resource(context::themes::regen, "Themes", "");
                        }
                    }
                }
            }

            if regenerated {
                self.context = context::load(self.cli.resolve_no_context_files(&self.cfg));
            }
        }

        // Recommended MCP prompts on first startup
        #[cfg(feature = "mcp")]
        if self.is_first_startup && self.is_interactive {
            let prompted =
                self.cfg.enable_context7_mcp.is_none() || self.cfg.enable_grepapp_mcp.is_none();
            if prompted {
                if self.cfg.enable_context7_mcp.is_none() {
                    let mut input = String::new();
                    eprint!("Enable Context7 MCP (documentation and code context lookup)? [y/N] ");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    std::io::stdin().read_line(&mut input)?;
                    let enable = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                    self.cfg.enable_context7_mcp = Some(enable);
                    if enable {
                        eprintln!("Context7 MCP enabled.");
                    }
                }
                if self.cfg.enable_grepapp_mcp.is_none() {
                    let mut input = String::new();
                    eprint!(
                        "Enable Grep.app MCP (semantic code search across repositories)? [y/N] "
                    );
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    std::io::stdin().read_line(&mut input)?;
                    let enable = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                    self.cfg.enable_grepapp_mcp = Some(enable);
                    if enable {
                        eprintln!("Grep.app MCP enabled.");
                    }
                }
                config::inject_mcp_defaults(&mut self.cfg);
                if let Err(e) = config::save_config(&self.cfg) {
                    tracing::warn!("Failed to save config with MCP choices: {e}");
                }
            }
        }

        // `SessionStart` fires here once `session_resumed` is known.
        #[cfg(feature = "hooks")]
        {
            let source = if self.session_resumed {
                "resume"
            } else {
                "startup"
            };
            crate::extras::hooks::dispatch_session_start(source).await;
        }

        // ARCHITECTURE.md prompt
        #[cfg(feature = "archmd")]
        let arch_created = if !self.cli.resolve_no_context_files(&self.cfg) {
            let cwd = std::env::current_dir().ok();
            if let Some(ref cwd) = cwd {
                crate::extras::archmd::ask_and_create(cwd).unwrap_or_else(|e| {
                    tracing::warn!("Architecture.md prompt failed: {e}");
                    false
                })
            } else {
                false
            }
        } else {
            false
        };

        // Reload context after potential ARCHITECTURE.md creation
        #[cfg(feature = "archmd")]
        if arch_created {
            self.context.architecture = crate::context::load_architecture();
        }

        // Default prompt resolution (after prompts may have been regenerated)
        {
            let default_prompt = self.cfg.default_prompt.as_deref().unwrap_or("code");
            if let Some(content) = self.context.prompts.get(default_prompt) {
                let (mode_directive, clean_content) = crate::permission::parse_prompt_mode(content);
                let mut prompt_text = if mode_directive.is_some() {
                    clean_content.to_string()
                } else {
                    content.clone()
                };

                let caps: &[&str] = &[
                    #[cfg(feature = "memory")]
                    "- **Memory**: persistent memory across sessions (memory_read, memory_write, memory_search)",
                    #[cfg(feature = "subagents")]
                    "- **Subagents**: delegate specific multi-step investigations to parallel subagents via the `task` tool",
                ];

                if !caps.is_empty() {
                    prompt_text.push_str("\n\n## Available Capabilities\n\n");
                    prompt_text.push_str(&caps.join("\n"));
                    prompt_text.push('\n');
                }

                self.context.current_prompt = Some(prompt_text);
                self.context.current_prompt_name = Some(default_prompt.to_string());

                apply_startup_prompt_model(
                    default_prompt,
                    &self.cfg,
                    &mut self.provider,
                    &mut self.model,
                    &mut self.session,
                );
            }
        }

        // --load-prompt overrides the default prompt
        if let Some(ref name) = self.cli.load_prompt {
            if let Some(content) = self.context.prompts.get(name) {
                let (mode_directive, clean_content) = crate::permission::parse_prompt_mode(content);
                let mut prompt_text = if mode_directive.is_some() {
                    clean_content.to_string()
                } else {
                    content.clone()
                };

                let caps: &[&str] = &[
                    #[cfg(feature = "memory")]
                    "- **Memory**: persistent memory across sessions (memory_read, memory_write, memory_search)",
                    #[cfg(feature = "subagents")]
                    "- **Subagents**: delegate specific multi-step investigations to parallel subagents via the `task` tool",
                ];

                if !caps.is_empty() {
                    prompt_text.push_str("\n\n## Available Capabilities\n\n");
                    prompt_text.push_str(&caps.join("\n"));
                    prompt_text.push('\n');
                }

                self.context.current_prompt = Some(prompt_text);
                self.context.current_prompt_name = Some(name.clone());

                apply_startup_prompt_model(
                    name,
                    &self.cfg,
                    &mut self.provider,
                    &mut self.model,
                    &mut self.session,
                );
            } else {
                let mut sorted: Vec<&String> = self.context.prompts.keys().collect();
                sorted.sort();
                eprintln!("error: unknown prompt '{}'", name);
                eprintln!("available prompts:");
                for p in &sorted {
                    eprintln!("  {}", p);
                }
                anyhow::bail!("unknown prompt '{}'", name);
            }
        }

        // Rebuild client if the provider changed due to prompt-to-model mapping
        if self.client.provider_name() != self.provider.as_str() {
            match provider::create_client(
                &self.provider,
                self.cli.api_key.as_deref(),
                &self.cfg.custom_providers_map(),
                self.cfg.api_keys.as_ref(),
            ) {
                Ok(new_client) => {
                    self.client = new_client;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to rebuild client for prompt-mapped provider '{}': {}",
                        self.provider,
                        e
                    );
                }
            }
        }

        // Apply mode from prompt %%mode= directive (if any).
        if let Some(perm) = &self.permission {
            let allowlist: Vec<(String, String)> = self
                .session
                .permission_allowlist
                .iter()
                .map(|e| (e.tool.to_string(), e.pattern.to_string()))
                .collect();
            let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
            guard.load_session_allowlist(&allowlist);
            if let Some(name) = &self.context.current_prompt_name
                && let Some(mode) =
                    crate::permission::resolve_startup_prompt_mode(&self.context.prompts, name)
            {
                guard.set_prompt_mode(mode);
            }
        }

        // Build the auto-trigger message for ARCHITECTURE.md creation
        #[cfg(feature = "archmd")]
        {
            self.arch_msg = if arch_created {
                Some(
                    "I've just created an empty ARCHITECTURE.md template at the project root. \
                    Explore the codebase thoroughly using the `task` tool (delegating parallel exploration to subagents) \
                    and fill ARCHITECTURE.md with a high-level architecture document covering:\n\
                    - Directory layout and module responsibilities\n\
                    - Key types, traits, and their relationships\n\
                    - Control flow (how requests/events flow through the system)\n\
                    - Data flow (how data is transformed from input to output)\n\
                    - Design decisions and rationale\n\
                    - External dependencies and how they are used\n\
                    - Entry points for different execution modes\n\n\
                    Keep the document under ~300 lines of code total. Keep entries concise and reference specific source files."
                        .to_string(),
                )
            } else {
                None
            };
        }

        Ok(())
    }

    /// Phase 4: mode dispatch — print, loop, or interactive.
    pub(crate) async fn dispatch(self) -> anyhow::Result<()> {
        if self.cli.print {
            self.dispatch_print().await
        } else {
            #[cfg(feature = "loop")]
            if self.cli.loop_mode {
                return self.dispatch_loop().await;
            }

            self.dispatch_interactive().await
        }
    }

    async fn dispatch_print(self) -> anyhow::Result<()> {
        let msg = self.cli.message.join(" ");
        if msg.starts_with('!') {
            let cmd = msg.strip_prefix('!').map(|s| s.trim()).unwrap_or("");
            if !cmd.is_empty() {
                let output = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(cmd)
                    .output()?;
                let mut result = String::new();
                if !output.stdout.is_empty() {
                    result.push_str(&String::from_utf8_lossy(&output.stdout));
                }
                if !output.stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&String::from_utf8_lossy(&output.stderr));
                }
                let result = result.trim().to_string();
                println!("{}", result);
                if !self.cli.no_session {
                    let mut session = self.session;
                    session.add_message(MessageRole::User, &msg);
                    session.add_message(MessageRole::Assistant, &result);
                    session::storage::save_session(&session)?;
                    if let Err(e) = session::chat_history::append_entry(
                        &session::chat_history::ChatHistoryEntry {
                            content: msg,
                            timestamp: session.updated_at.clone(),
                        },
                    ) {
                        eprintln!("warning: failed to append chat history entry: {}", e);
                    }
                }
            } else {
                eprintln!("error: empty command after '!'");
            }
        } else {
            let temperature = config::resolve_temperature(&self.cli, &self.cfg, &self.model);
            let extra_body = config::resolve_extra_body(&self.cfg, &self.model);
            let completion_model = self.client.completion_model(self.model.to_string());
            #[cfg(feature = "mcp")]
            let mcp_manager = connect_headless_mcp(&self.cfg).await;
            let agent = provider::build_agent(
                completion_model,
                &self.cli,
                &self.cfg,
                &self.context,
                self.permission,
                // Non-interactive dispatch never keeps the ask channel: with
                // no one draining it, an `Ask` verdict must fail closed as a
                // denial rather than block forever. See `handle_ask_inner`.
                None,
                self.sandbox.clone(),
                true,
                temperature,
                extra_body,
                #[cfg(feature = "mcp")]
                mcp_manager.as_ref(),
            )
            .await;
            #[cfg(feature = "advisor")]
            {
                let mut msgs = self.session.messages.clone();
                msgs.push(SessionMessage {
                    role: MessageRole::User,
                    content: CompactString::new(&msg),
                    estimated_tokens: Session::estimate_tokens(&msg),
                });
                crate::extras::advisor::set_session_messages(msgs);
            }
            if let Some(ss) = self.status_signals.as_ref() {
                ss.send_start();
            }
            let history = crate::agent::runner::convert_history(&self.session);
            let response_result = agent
                .run_print(
                    &msg,
                    self.cli.pure_stdout,
                    &self.cfg.retry,
                    history,
                    #[cfg(feature = "hooks")]
                    None,
                )
                .await;
            if let Some(ss) = self.status_signals.as_ref() {
                ss.send_stop();
            }
            let (response, usage) = response_result?;
            if !self.cli.no_session {
                let mut session = self.session;
                session.add_message(MessageRole::User, &msg);
                session.add_message(MessageRole::Assistant, &response);
                session.total_input_tokens = session
                    .total_input_tokens
                    .saturating_add(usage.input_tokens);
                session.total_output_tokens = session
                    .total_output_tokens
                    .saturating_add(usage.output_tokens);
                session.total_cached_input_tokens = session
                    .total_cached_input_tokens
                    .saturating_add(usage.cached_input_tokens);
                session.total_cache_creation_input_tokens = session
                    .total_cache_creation_input_tokens
                    .saturating_add(usage.cache_creation_input_tokens);
                session.total_cost += crate::pricing::estimate_cost(
                    crate::pricing::billable_input_tokens(
                        self.cfg.is_anthropic_native(&session.provider),
                        usage.input_tokens,
                        usage.cached_input_tokens,
                        usage.cache_creation_input_tokens,
                    ),
                    usage.output_tokens,
                    session.input_token_cost,
                    session.output_token_cost,
                );
                session::storage::save_session(&session)?;
                let _ =
                    session::chat_history::append_entry(&session::chat_history::ChatHistoryEntry {
                        content: msg,
                        timestamp: session.updated_at.clone(),
                    });
            }
        }

        #[cfg(feature = "hooks")]
        crate::extras::hooks::dispatch_session_end("exit").await;

        Ok(())
    }

    #[cfg(feature = "loop")]
    async fn dispatch_loop(self) -> anyhow::Result<()> {
        let model_completion = self.client.completion_model(self.model.to_string());
        let temperature = config::resolve_temperature(&self.cli, &self.cfg, &self.model);
        let extra_body = config::resolve_extra_body(&self.cfg, &self.model);
        #[cfg(feature = "mcp")]
        let mcp_manager = connect_headless_mcp(&self.cfg).await;
        let agent = provider::build_agent(
            model_completion,
            &self.cli,
            &self.cfg,
            &self.context,
            self.permission,
            // Non-interactive dispatch never keeps the ask channel; see the
            // matching note in `dispatch_print`.
            None,
            self.sandbox.clone(),
            true,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            mcp_manager.as_ref(),
        )
        .await;
        let result = crate::extras::r#loop::headless::run_headless_loop(
            agent,
            &self.cli,
            &self.cfg,
            &self.context,
            self.status_signals,
        )
        .await;
        #[cfg(feature = "hooks")]
        crate::extras::hooks::dispatch_session_end("exit").await;
        result
    }

    async fn dispatch_interactive(self) -> anyhow::Result<()> {
        let Startup {
            cli,
            cfg,
            mut session,
            mut context,
            client,
            permission,
            ask_tx,
            ask_rx,
            sandbox,
            status_signals,
            arch_msg,
            #[cfg(feature = "advisor")]
            handoff_rx,
            ..
        } = self;

        let initial_msg = cli.message.join(" ");
        if !initial_msg.is_empty() {
            session.add_message(MessageRole::User, &initial_msg);
        }

        crate::ui::run_interactive(
            crate::ui::state::UiContext::new(
                &cli,
                &cfg,
                &mut session,
                &mut context,
                client,
                permission,
                ask_tx,
                sandbox,
                status_signals,
            ),
            None,
            ask_rx,
            arch_msg,
            #[cfg(feature = "advisor")]
            handoff_rx,
        )
        .await?;

        #[cfg(feature = "hooks")]
        crate::extras::hooks::dispatch_session_end("exit").await;

        Ok(())
    }
}
