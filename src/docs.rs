use std::path::{Path, PathBuf};

/// The docs the binary actually reads at runtime. Website-only assets
/// (`index.html`, `template.html`, `robots.txt`, `providers/*.md`) and the
/// unreferenced `PUBLISHING_RELEASES.md` / `STATUS_SIGNALS.md` are deliberately
/// left out so they are not baked into the binary.
const EMBEDDED: &[(&str, &str)] = &[
    ("GET_STARTED.md", include_str!("../docs/GET_STARTED.md")),
    ("COMMANDS.md", include_str!("../docs/COMMANDS.md")),
    ("CONFIG.md", include_str!("../docs/CONFIG.md")),
    ("PROVIDERS.md", include_str!("../docs/PROVIDERS.md")),
    ("HASHEDIT.md", include_str!("../docs/HASHEDIT.md")),
    ("MEMORY.md", include_str!("../docs/MEMORY.md")),
    ("ARCHITECTURE.md", include_str!("../docs/ARCHITECTURE.md")),
    ("SUBAGENTS.md", include_str!("../docs/SUBAGENTS.md")),
];

pub fn global_docs_dir() -> PathBuf {
    crate::session::storage::data_dir().join("docs")
}

pub fn show_get_started() -> anyhow::Result<()> {
    ensure_global()?;
    let doc_path = global_docs_dir().join("GET_STARTED.md");
    if !doc_path.exists() {
        anyhow::bail!(
            "GET_STARTED.md not found at {}. Try reinstalling zerostack.",
            doc_path.display()
        );
    }
    let status = std::process::Command::new("less").arg(&doc_path).status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub fn ensure_global() -> anyhow::Result<bool> {
    let dir = global_docs_dir();
    let version_file = dir.join("current_version");
    let current_version = env!("CARGO_PKG_VERSION");

    let should_copy = match std::fs::read_to_string(&version_file) {
        Ok(stored) => stored.trim() != current_version,
        Err(_) => true,
    };

    if should_copy {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        copy_embedded(&dir)?;
        std::fs::write(&version_file, current_version)?;
        return Ok(true);
    }

    Ok(false)
}

fn copy_embedded(dest: &Path) -> anyhow::Result<()> {
    for (name, content) in EMBEDDED {
        std::fs::write(dest.join(name), content)?;
    }
    Ok(())
}
