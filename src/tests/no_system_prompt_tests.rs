//! `--no-system-prompt` CLI flag parsing.

use clap::Parser;

use crate::cli::Cli;

fn cli_with(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("zerostack").chain(args.iter().copied())).unwrap()
}

#[test]
fn flag_defaults_to_off() {
    assert!(!Cli::default().no_system_prompt);
}

#[test]
fn flag_parses() {
    assert!(cli_with(&["--no-system-prompt"]).no_system_prompt);
}

#[test]
fn conflicts_with_load_prompt() {
    assert!(
        Cli::try_parse_from(["zerostack", "--no-system-prompt", "--load-prompt", "code",]).is_err()
    );
}
