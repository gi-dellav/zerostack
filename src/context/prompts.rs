use std::collections::HashMap;
use std::path::PathBuf;

use include_dir::{Dir, include_dir};

static EMBEDDED: Dir = include_dir!("$CARGO_MANIFEST_DIR/data/prompts");

/// Which source a prompt was loaded from. Later sources override earlier
/// ones for same-named prompts, so each prompt's source is the
/// highest-priority location that defines it (see `load_with_sources`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSource {
    /// Compiled into the binary (`data/prompts` at build time).
    Embedded,
    /// User-level data dir (`~/.local/share/zerostack/prompts/`).
    Global,
    /// Project-local `data/prompts/`, relative to the CWD.
    DataDir,
    /// Project-level `.zerostack/prompts/` (highest priority).
    Project,
}

impl PromptSource {
    /// Short tag shown next to the prompt name in pickers and `/prompt`.
    /// Both project-level sources display as `local`.
    pub fn label(&self) -> &'static str {
        match self {
            PromptSource::Embedded => "built-in",
            PromptSource::Global => "global",
            PromptSource::DataDir | PromptSource::Project => "local",
        }
    }
}

pub fn global_dir() -> PathBuf {
    crate::session::storage::data_dir().join("prompts")
}

pub fn zerostack_dir() -> PathBuf {
    PathBuf::from(".zerostack/prompts")
}

/// Load all prompts together with the source each one came from. Sources are
/// scanned in priority order (low to high); a later source overrides both the
/// content and the provenance of a same-named prompt. Within the embedded
/// set, the first file with a given name wins.
pub fn load_with_sources() -> (HashMap<String, String>, HashMap<String, PromptSource>) {
    let mut prompts: HashMap<String, String> = HashMap::new();
    let mut sources: HashMap<String, PromptSource> = HashMap::new();

    for (name, content) in crate::context::load_embedded_files(&EMBEDDED, "md") {
        prompts.entry(name.clone()).or_insert(content);
        sources.entry(name).or_insert(PromptSource::Embedded);
    }
    for (name, content) in crate::context::load_dir_files(&global_dir(), "md") {
        sources.insert(name.clone(), PromptSource::Global);
        prompts.insert(name, content);
    }
    for (name, content) in crate::context::load_dir_files(&PathBuf::from("data/prompts"), "md") {
        sources.insert(name.clone(), PromptSource::DataDir);
        prompts.insert(name, content);
    }
    for (name, content) in crate::context::load_dir_files(&zerostack_dir(), "md") {
        sources.insert(name.clone(), PromptSource::Project);
        prompts.insert(name, content);
    }

    (prompts, sources)
}

pub fn ensure_global() -> anyhow::Result<()> {
    let dir = global_dir();
    if !dir.exists() {
        crate::context::copy_embedded_to(&EMBEDDED, &dir)?;
    }
    Ok(())
}

pub fn regen() -> anyhow::Result<()> {
    let dir = global_dir();
    crate::context::copy_embedded_to(&EMBEDDED, &dir)
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    struct TestDir {
        dir: PathBuf,
        orig_cwd: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TestDir {
        fn new() -> Self {
            let lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("zs_pr_test_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            unsafe {
                std::env::set_var("ZS_DATA_DIR", dir.to_str().unwrap());
            }
            let orig_cwd = std::env::current_dir().unwrap();
            std::env::set_current_dir(&dir).unwrap();
            TestDir {
                dir,
                orig_cwd,
                _lock: lock,
            }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.orig_cwd);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn write_prompt(path: &PathBuf, name: &str, content: &str) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join(format!("{}.md", name)), content).unwrap();
    }

    #[test]
    fn test_zerostack_prompts_are_loaded() {
        let _td = TestDir::new();
        let dir = zerostack_dir();
        write_prompt(&dir, "myproject", "# My Project Prompt");

        let prompts = load_with_sources().0;
        assert!(prompts.contains_key("myproject"));
        assert_eq!(prompts["myproject"], "# My Project Prompt");
    }

    #[test]
    fn test_zerostack_overrides_prompts_dir() {
        let _td = TestDir::new();
        let prompts_dir = PathBuf::from("data/prompts");
        let zs_dir = zerostack_dir();
        write_prompt(&prompts_dir, "code", "from prompts/");
        write_prompt(&zs_dir, "code", "from .zerostack/prompts/");

        let prompts = load_with_sources().0;
        assert_eq!(prompts["code"], "from .zerostack/prompts/");
    }

    #[test]
    fn test_zerostack_overrides_global() {
        let _td = TestDir::new();
        let global = global_dir();
        let zs_dir = zerostack_dir();
        write_prompt(&global, "code", "from global/");
        write_prompt(&zs_dir, "code", "from .zerostack/");

        let prompts = load_with_sources().0;
        assert_eq!(prompts["code"], "from .zerostack/");
    }

    #[test]
    fn test_zerostack_overrides_embedded() {
        let _td = TestDir::new();
        let zs_dir = zerostack_dir();
        write_prompt(&zs_dir, "code", "from .zerostack/");

        let prompts = load_with_sources().0;
        assert_eq!(prompts["code"], "from .zerostack/");
    }

    #[test]
    fn test_prompts_dir_overrides_global() {
        let _td = TestDir::new();
        let global = global_dir();
        let prompts_dir = PathBuf::from("data/prompts");
        write_prompt(&global, "custom", "from global/");
        write_prompt(&prompts_dir, "custom", "from prompts/");

        let prompts = load_with_sources().0;
        assert_eq!(prompts["custom"], "from prompts/");
    }

    #[test]
    fn test_full_priority_chain() {
        let _td = TestDir::new();
        let global = global_dir();
        let prompts_dir = PathBuf::from("data/prompts");
        let zs_dir = zerostack_dir();

        write_prompt(&global, "code", "from global/");
        write_prompt(&prompts_dir, "custom", "from prompts/");
        write_prompt(&zs_dir, "custom", "from .zerostack/");
        write_prompt(&zs_dir, "code", "from .zerostack/code");

        let prompts = load_with_sources().0;
        assert_eq!(prompts["code"], "from .zerostack/code");
        assert_eq!(prompts["custom"], "from .zerostack/");
        assert!(prompts.contains_key("ask"));
    }

    #[test]
    fn test_zerostack_dir_missing_is_ok() {
        let _td = TestDir::new();
        let prompts = load_with_sources().0;
        assert!(prompts.contains_key("code"));
        assert!(prompts.contains_key("ask"));
        assert!(prompts.contains_key("default"));
    }

    #[test]
    fn test_source_labels() {
        assert_eq!(PromptSource::Embedded.label(), "built-in");
        assert_eq!(PromptSource::Global.label(), "global");
        assert_eq!(PromptSource::DataDir.label(), "local");
        assert_eq!(PromptSource::Project.label(), "local");
    }

    #[test]
    fn test_embedded_prompt_source() {
        let _td = TestDir::new();
        let (_, sources) = load_with_sources();
        assert_eq!(sources.get("code"), Some(&PromptSource::Embedded));
    }

    #[test]
    fn test_global_override_source() {
        let _td = TestDir::new();
        write_prompt(&global_dir(), "code", "from global/");
        let (prompts, sources) = load_with_sources();
        assert_eq!(prompts["code"], "from global/");
        assert_eq!(sources.get("code"), Some(&PromptSource::Global));
    }

    #[test]
    fn test_data_dir_override_source() {
        let _td = TestDir::new();
        write_prompt(&PathBuf::from("data/prompts"), "code", "from data/");
        let (_, sources) = load_with_sources();
        assert_eq!(sources.get("code"), Some(&PromptSource::DataDir));
    }

    #[test]
    fn test_project_override_source() {
        let _td = TestDir::new();
        write_prompt(&zerostack_dir(), "code", "from .zerostack/");
        let (_, sources) = load_with_sources();
        assert_eq!(sources.get("code"), Some(&PromptSource::Project));
    }

    #[test]
    fn test_source_tracks_winning_override() {
        let _td = TestDir::new();
        write_prompt(&global_dir(), "mixed", "from global/");
        write_prompt(&zerostack_dir(), "mixed", "from .zerostack/");
        let (prompts, sources) = load_with_sources();
        assert_eq!(prompts["mixed"], "from .zerostack/");
        assert_eq!(sources.get("mixed"), Some(&PromptSource::Project));
    }
}
