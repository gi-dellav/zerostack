use std::io::Write;
use std::path::{Path, PathBuf};

const DIRS_ASKED_FILE: &str = "dirs_asked_architecture.txt";

fn dirs_asked_path() -> PathBuf {
    let dir = crate::session::storage::data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("archmd: failed to create data dir: {e}");
    }
    dir.join(DIRS_ASKED_FILE)
}

const ARCHITECTURE_TEMPLATE: &str = "# Architecture

This file documents the high-level architecture of the codebase.
It helps AI agents understand the project structure, key modules,
data flow, and design decisions without having to re-discover
them on every exploration.

## Contents to include

- **Directory layout** — top-level modules and their responsibilities
- **Key types and traits** — central abstractions and their relationships
- **Control flow** — how requests/events flow through the system
- **Data flow** — how data is transformed from input to output
- **Design decisions** — why certain approaches were chosen
- **Dependencies** — external libraries and how they are used
- **Entry points** — where execution starts for different modes

## Instructions for agents

When you discover new architectural information about this
codebase, add it to this file so future explorations start
with the full picture. Keep the document under ~300 lines
    of code total. Keep entries concise and link to specific source files.
";

// ---------------------------------------------------------------------------
// Public API (uses global asked_path)
// ---------------------------------------------------------------------------

pub(crate) fn record_asked_dir(dir: &Path) -> anyhow::Result<()> {
    record_asked_dir_with_path(dir, &dirs_asked_path())
}

pub fn should_ask(dir: &Path) -> bool {
    should_ask_with_path(dir, &dirs_asked_path())
}

pub(crate) fn should_ask_with_path(dir: &Path, asked_path: &Path) -> bool {
    let arch_path = dir.join("ARCHITECTURE.md");
    !arch_path.exists() && !has_been_asked_with_path(dir, asked_path)
}

pub fn ask_and_create(dir: &Path) -> anyhow::Result<bool> {
    if !should_ask(dir) {
        return Ok(false);
    }

    eprint!(
        "No ARCHITECTURE.md found in {} (documents high-level codebase architecture for AI agents). Create one? [y/N] ",
        dir.display()
    );
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if let Err(e) = record_asked_dir(dir) {
        tracing::warn!("Failed to record asked directory: {e}");
    }

    if input == "y" || input == "yes" {
        create_architecture_template(dir)?;
        eprintln!(
            "Created {}/ARCHITECTURE.md — edit it to describe the codebase architecture.",
            dir.display()
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Inner functions with explicit asked_path (testable)
// ---------------------------------------------------------------------------

pub(crate) fn has_been_asked_with_path(dir: &Path, asked_path: &Path) -> bool {
    let canonical = dir.canonicalize().ok();
    let target = canonical.as_deref().unwrap_or(dir);
    if !asked_path.exists() {
        return false;
    }
    let content = std::fs::read_to_string(asked_path).unwrap_or_default();
    for line in content.lines() {
        if let Ok(asked_dir) = PathBuf::from(line).canonicalize()
            && asked_dir == target
        {
            return true;
        }
    }
    false
}

pub(crate) fn record_asked_dir_with_path(dir: &Path, asked_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = asked_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir_str = dir.to_string_lossy().to_string();
    let mut content = String::new();
    if asked_path.exists() {
        content = std::fs::read_to_string(asked_path).unwrap_or_default();
    }
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(&dir_str);
    content.push('\n');
    let tmp = asked_path.with_extension("tmp");
    std::fs::write(&tmp, &content)?;
    std::fs::rename(&tmp, asked_path)?;
    Ok(())
}

pub(crate) fn create_architecture_template(dir: &Path) -> anyhow::Result<()> {
    let path = dir.join("ARCHITECTURE.md");
    if path.exists() {
        return Ok(());
    }
    std::fs::write(&path, ARCHITECTURE_TEMPLATE)?;
    Ok(())
}
