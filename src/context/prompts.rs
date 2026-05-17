use std::collections::HashMap;
use std::path::PathBuf;

use include_dir::{Dir, include_dir};

static EMBEDDED: Dir = include_dir!("$CARGO_MANIFEST_DIR/prompts");

pub fn global_prompts_dir() -> PathBuf {
    crate::session::storage::config_path().join("prompts")
}

pub fn load() -> HashMap<String, String> {
    let mut prompts: HashMap<String, String> = HashMap::new();

    for file in EMBEDDED.files() {
        if file.path().extension().is_some_and(|e| e == "md")
            && let Some(name) = file.path().file_stem().and_then(|s| s.to_str())
            && let Some(content) = file.contents_utf8()
        {
            prompts
                .entry(name.to_string())
                .or_insert_with(|| content.to_string());
        }
    }

    let global = global_prompts_dir();
    if global.exists()
        && let Ok(entries) = std::fs::read_dir(&global)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                prompts.insert(name.to_string(), content);
            }
        }
    }

    let local = PathBuf::from("prompts");
    if local.exists()
        && let Ok(entries) = std::fs::read_dir(&local)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                prompts.insert(name.to_string(), content);
            }
        }
    }

    prompts
}

pub fn ensure_global() -> anyhow::Result<()> {
    let dir = global_prompts_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        copy_embedded(&dir)?;
    }
    Ok(())
}

pub fn regen() -> anyhow::Result<()> {
    let dir = global_prompts_dir();
    std::fs::create_dir_all(&dir)?;
    copy_embedded(&dir)
}

fn copy_embedded(dest: &PathBuf) -> anyhow::Result<()> {
    for file in EMBEDDED.files() {
        if let Some(name) = file.path().file_name().and_then(|s| s.to_str()) {
            let dest_path = dest.join(name);
            if let Some(content) = file.contents_utf8() {
                std::fs::write(&dest_path, content)?;
            }
        }
    }
    Ok(())
}
