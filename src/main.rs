mod agent;
mod cli;
mod config;
mod context;
mod event;
mod session;
mod ui;

use clap::Parser;
use rig::client::CompletionClient;
use session::MessageRole;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = cli::Cli::parse();
    let cfg = config::load();
    let context = context::load(cli.resolve_no_context_files(&cfg));

    let provider = cli.resolve_provider(&cfg);
    let model = cli.resolve_model(&cfg);

    let mut session = session::Session::new(&provider, &model);

    if cli.resume && cli.session.is_none() && !cli.continue_session {
        let sessions = session::storage::find_recent_sessions(10)?;
        if sessions.is_empty() {
            eprintln!("No recent sessions found.");
        } else {
            eprintln!("Recent sessions:");
            for (i, s) in sessions.iter().enumerate() {
                let preview = s.messages.last().map(|m| {
                    let truncated: String = m.content.chars().take(60).collect();
                    truncated
                }).unwrap_or_default();
                eprintln!("  {}. {}  [{} msgs] {}",
                    i + 1, &s.id[..8], s.messages.len(), preview);
            }
            if let Some(s) = sessions.into_iter().next() {
                session = s;
            }
        }
    }

    if cli.continue_session
        && cli.session.is_none()
        && let Ok(sessions) = session::storage::find_recent_sessions(1)
        && let Some(s) = sessions.into_iter().next()
    {
        session = s;
    }

    if let Some(session_id) = &cli.session {
        session = session::storage::load_session(session_id)?;
    }

    let client = agent::create_client(cli.api_key.as_deref())?;
    let completion_model = client.completion_model(model.to_string());
    let agent = agent::build_agent(completion_model, &cli, &cfg, &context);

    if cli.print {
        let msg = cli.message.join(" ");
        let response = agent::run_print(&agent, &msg).await?;
        if !cli.no_session {
            session.add_message(MessageRole::User, &msg);
            session.add_message(MessageRole::Assistant, &response);
            session::storage::save_session(&session)?;
        }
    } else {
        let initial_msg = cli.message.join(" ");
        if !initial_msg.is_empty() {
            session.add_message(MessageRole::User, &initial_msg);
        }
        ui::run_interactive(client, agent, &cli, &cfg, &mut session, &context).await?;
    }

    Ok(())
}
