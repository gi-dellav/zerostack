use std::collections::HashMap;
use std::path::{Path, PathBuf};

use include_dir::Dir;
use smallvec::SmallVec;

use crate::session::storage;

pub mod prompts;
pub mod themes;

pub(crate) fn load_embedded_files(embedded: &Dir, ext: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for file in embedded.files() {
        if file.path().extension().is_some_and(|e| e == ext)
            && let Some(name) = file.path().file_stem().and_then(|s| s.to_str())
            && let Some(content) = file.contents_utf8()
        {
            results.push((name.to_string(), content.to_string()));
        }
    }
    results
}

pub(crate) fn load_dir_files(dir: &Path, ext: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    if dir.exists()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == ext)
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                results.push((name.to_string(), content));
            }
        }
    }
    results
}

pub(crate) fn copy_embedded_to(embedded: &Dir, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    for file in embedded.files() {
        if let Some(name) = file.path().file_name().and_then(|s| s.to_str()) {
            let dest_path = dest.join(name);
            if let Some(content) = file.contents_utf8() {
                std::fs::write(&dest_path, content)?;
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct ContextFiles {
    pub agents: Option<String>,
    pub prompts: HashMap<String, String>,
    pub current_prompt: Option<String>,
    pub current_prompt_name: Option<String>,
    pub themes: HashMap<String, String>,
    pub current_theme_name: Option<String>,
    pub extra_files: Vec<std::path::PathBuf>,
    pub one_shot_restore: Option<String>,
    pub chain_declined: Vec<String>,
    #[cfg(feature = "memory")]
    pub memory: Option<String>,
    #[cfg(feature = "archmd")]
    pub architecture: Option<String>,
}

impl ContextFiles {
    #[cfg(feature = "git-worktree")]
    pub fn reload(&mut self) {
        self.agents = walk_context_files().0;
        #[cfg(feature = "archmd")]
        {
            self.architecture = walk_context_files().1;
        }
        self.prompts = prompts::load();
        if let Some(name) = &self.current_prompt_name {
            self.current_prompt = self.prompts.get(name).cloned();
        }
        self.themes = themes::load();
        self.current_theme_name = crate::session::storage::load_theme_name();
        #[cfg(feature = "memory")]
        {
            self.memory = crate::extras::memory::Mem::open().context_block();
        }
    }
}

pub fn load(no_context_files: bool) -> ContextFiles {
    if let Err(e) = prompts::ensure_global() {
        tracing::warn!("failed to install default prompts: {e}");
    }
    if let Err(e) = themes::ensure_global() {
        tracing::warn!("failed to install default themes: {e}");
    }
    let (agents, arch_candidate) = if no_context_files {
        (None, None)
    } else {
        walk_context_files()
    };
    #[cfg(feature = "archmd")]
    let architecture = arch_candidate;
    #[cfg(not(feature = "archmd"))]
    let _ = arch_candidate;
    let prompt_map = prompts::load();
    let theme_map = themes::load();
    let theme_name = crate::session::storage::load_theme_name();
    #[cfg(feature = "memory")]
    let memory = crate::extras::memory::Mem::open().context_block();
    ContextFiles {
        agents,
        prompts: prompt_map,
        current_prompt: None,
        current_prompt_name: None,
        themes: theme_map,
        current_theme_name: theme_name,
        extra_files: Vec::new(),
        one_shot_restore: None,
        chain_declined: Vec::new(),
        #[cfg(feature = "memory")]
        memory,
        #[cfg(feature = "archmd")]
        architecture,
    }
}

fn load_file(path: &PathBuf) -> Option<String> {
    if path.exists() {
        std::fs::read_to_string(path).ok()
    } else {
        None
    }
}

/// Maximum total bytes of ancestor context files (AGENTS.md, CLAUDE.md,
/// ARCHITECTURE.md) to load into the system prompt. Prevents a planted or
/// oversized file from blowing up the context window.
const MAX_ANCESTOR_CONTEXT_BYTES: usize = 524_288;

/// Walks from CWD up to root once, collecting AGENTS.md, CLAUDE.md, and
/// ARCHITECTURE.md files. This avoids the duplicate traversal that the
/// older separate load_agents / load_architecture performed.
fn walk_context_files() -> (Option<String>, Option<String>) {
    let mut agent_parts: SmallVec<[String; 4]> = SmallVec::new();
    #[cfg_attr(not(feature = "archmd"), allow(unused_mut))]
    let mut arch_parts: SmallVec<[String; 4]> = SmallVec::new();
    let mut total_bytes: usize = 0;

    let global_agents = storage::agents_path();
    if let Some(content) = load_file(&global_agents)
        && !content.trim().is_empty()
    {
        total_bytes += content.len();
        agent_parts.push(format!("# Global AGENTS.md\n{}", content));
    }

    #[cfg(feature = "archmd")]
    {
        let global_arch = storage::architecture_path();
        if let Some(content) = load_file(&global_arch)
            && !content.trim().is_empty()
        {
            total_bytes += content.len();
            arch_parts.push(format!("# Global ARCHITECTURE.md\n{}", content));
        }
    }

    let cwd = std::env::current_dir().ok();
    if let Some(cwd) = cwd {
        let mut current = Some(cwd.as_path());
        while let Some(dir) = current {
            if total_bytes >= MAX_ANCESTOR_CONTEXT_BYTES {
                tracing::warn!(
                    "ancestor context files exceed {} bytes, stopping traversal",
                    MAX_ANCESTOR_CONTEXT_BYTES
                );
                break;
            }
            for name in &["AGENTS.md", "CLAUDE.md"] {
                let path = dir.join(name);
                if let Some(content) = load_file(&path)
                    && !content.trim().is_empty()
                {
                    total_bytes += content.len();
                    agent_parts.push(format!("# {} ({})\n{}", name, dir.display(), content));
                }
            }
            #[cfg(feature = "archmd")]
            {
                let path = dir.join("ARCHITECTURE.md");
                if let Some(content) = load_file(&path)
                    && !content.trim().is_empty()
                {
                    total_bytes += content.len();
                    arch_parts.push(format!(
                        "# ARCHITECTURE.md ({})\n{}",
                        dir.display(),
                        content
                    ));
                }
            }
            current = dir.parent();
        }
    }

    let agents = if agent_parts.is_empty() {
        None
    } else {
        Some(agent_parts.join("\n\n"))
    };
    let architecture = if arch_parts.is_empty() {
        None
    } else {
        Some(arch_parts.join("\n\n"))
    };
    (agents, architecture)
}

#[cfg(feature = "archmd")]
pub(crate) fn load_architecture() -> Option<String> {
    walk_context_files().1
}
