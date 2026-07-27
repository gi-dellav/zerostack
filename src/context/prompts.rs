use std::collections::HashMap;
use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};

static EMBEDDED: Dir = include_dir!("$CARGO_MANIFEST_DIR/data/prompts");

pub fn global_dir() -> PathBuf {
    crate::session::storage::data_dir().join("prompts")
}

pub fn zerostack_dir() -> PathBuf {
    PathBuf::from(".zerostack/prompts")
}

pub fn load() -> HashMap<String, String> {
    let mut prompts: HashMap<String, String> = HashMap::new();

    for (name, content) in crate::context::load_embedded_files(&EMBEDDED, "md") {
        prompts.entry(name).or_insert(content);
    }
    for (name, content) in crate::context::load_dir_files(&global_dir(), "md") {
        prompts.insert(name, content);
    }
    for (name, content) in crate::context::load_dir_files(&PathBuf::from("data/prompts"), "md") {
        prompts.insert(name, content);
    }
    for (name, content) in crate::context::load_dir_files(&zerostack_dir(), "md") {
        prompts.insert(name, content);
    }

    prompts
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
    crate::context::copy_embedded_to(&EMBEDDED, &dir)?;
    Ok(())
}

/// Names of embedded prompt files that are missing or modified in `dir`.
pub fn changed_files(dir: &Path) -> Vec<String> {
    crate::context::embedded_changed_files(&EMBEDDED, dir)
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

        let prompts = load();
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

        let prompts = load();
        assert_eq!(prompts["code"], "from .zerostack/prompts/");
    }

    #[test]
    fn test_zerostack_overrides_global() {
        let _td = TestDir::new();
        let global = global_dir();
        let zs_dir = zerostack_dir();
        write_prompt(&global, "code", "from global/");
        write_prompt(&zs_dir, "code", "from .zerostack/");

        let prompts = load();
        assert_eq!(prompts["code"], "from .zerostack/");
    }

    #[test]
    fn test_zerostack_overrides_embedded() {
        let _td = TestDir::new();
        let zs_dir = zerostack_dir();
        write_prompt(&zs_dir, "code", "from .zerostack/");

        let prompts = load();
        assert_eq!(prompts["code"], "from .zerostack/");
    }

    #[test]
    fn test_prompts_dir_overrides_global() {
        let _td = TestDir::new();
        let global = global_dir();
        let prompts_dir = PathBuf::from("data/prompts");
        write_prompt(&global, "custom", "from global/");
        write_prompt(&prompts_dir, "custom", "from prompts/");

        let prompts = load();
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

        let prompts = load();
        assert_eq!(prompts["code"], "from .zerostack/code");
        assert_eq!(prompts["custom"], "from .zerostack/");
        assert!(prompts.contains_key("ask"));
    }

    #[test]
    fn test_zerostack_dir_missing_is_ok() {
        let _td = TestDir::new();
        let prompts = load();
        assert!(prompts.contains_key("code"));
        assert!(prompts.contains_key("ask"));
        assert!(prompts.contains_key("default"));
    }
}
