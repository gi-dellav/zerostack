use std::path::PathBuf;

use uuid::Uuid;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::extras::r#loop as loop_mod;
use crate::extras::status_signals::StatusSignals;
use crate::provider::AnyAgent;

pub(crate) async fn run_headless_loop(
    agent: AnyAgent,
    cli: &Cli,
    cfg: &Config,
    _context: &ContextFiles,
    status_signals: Option<StatusSignals>,
) -> anyhow::Result<()> {
    let prompt = cli
        .loop_prompt
        .clone()
        .or_else(|| {
            let msg = cli.message.join(" ");
            if msg.is_empty() { None } else { Some(msg) }
        })
        .ok_or_else(|| anyhow::anyhow!("No loop prompt. Use --loop-prompt or pass a message."))?;

    let plan_file = cli
        .loop_plan
        .clone()
        .unwrap_or_else(|| PathBuf::from(loop_mod::DEFAULT_PLAN_FILENAME));
    let max_iterations = cli.loop_max;
    let run_cmd = cli.loop_run.clone();
    let session_id = Uuid::new_v4().to_string();

    let use_existing = loop_mod::plan::handle_startup(&plan_file).await?;
    if !use_existing {
        // No plan exists — agent will generate one on first iteration
    }

    let mut state = loop_mod::LoopState::new(prompt, plan_file, max_iterations, run_cmd);

    loop {
        state.iteration += 1;

        if state.should_stop() {
            eprintln!(
                "[loop] max iterations ({}) reached, stopping",
                state.max_iterations.unwrap_or(0)
            );
            break;
        }

        let iteration_prompt = state.build_prompt();

        eprintln!("=== {} ===", state.iteration_label());
        eprintln!();

        if let Some(ss) = status_signals.as_ref() {
            ss.send_start();
        }
        let response = match agent
            .run_print(
                &iteration_prompt,
                cli.pure_stdout,
                &cfg.retry,
                // TODO(loop-history): the loop threads context only through the
                // plan/summary state folded into `iteration_prompt`, so every
                // iteration sends empty history. Making `--loop --continue`
                // meaningful needs the loop to carry a real `Session`, a
                // separate change.
                Vec::new(),
                #[cfg(feature = "hooks")]
                Some(crate::extras::hooks::LoopInfo {
                    iteration: state.iteration,
                    active: state.active,
                }),
            )
            .await
        {
            Ok((r, _usage)) => {
                if let Some(ss) = status_signals.as_ref() {
                    ss.send_stop();
                }
                r
            }
            Err(e) => {
                if let Some(ss) = status_signals.as_ref() {
                    ss.send_stop();
                }
                eprintln!("[loop] error in iteration {}: {}", state.iteration, e);
                break;
            }
        };

        let summary: String = response
            .chars()
            .take(loop_mod::SUMMARY_TRUNCATION_CHARS)
            .collect();
        state.last_summary = Some(summary.clone());

        let validation_output = if let Some(cmd) = &state.run_cmd {
            eprintln!("--- Validation: {} ---", cmd);
            let shell = if cfg!(windows) { "powershell" } else { "sh" };
            let shell_arg = if cfg!(windows) { "-Command" } else { "-c" };
            match tokio::process::Command::new(shell)
                .arg(shell_arg)
                .arg(cmd)
                .output()
                .await
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let combined = if stderr.is_empty() {
                        stdout
                    } else {
                        format!("{}\n{}", stdout, stderr)
                    };
                    eprintln!("{}", combined);
                    Some(combined)
                }
                Err(e) => {
                    let msg = format!("error: {}", e);
                    eprintln!("{}", msg);
                    Some(msg)
                }
            }
        } else {
            None
        };
        state.last_run_output = validation_output.clone();

        if let Err(e) = loop_mod::transcript::save_iteration(
            &session_id,
            state.iteration,
            &iteration_prompt,
            &response,
            validation_output.as_deref(),
            &summary,
        ) {
            eprintln!("[loop] warning: failed to save transcript: {}", e);
        }

        eprintln!("--- iteration {} complete, looping ---\n", state.iteration);
    }

    Ok(())
}
