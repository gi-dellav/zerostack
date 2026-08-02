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

/// Whether prompt `name`'s currently-loaded content is the compiled-in
/// default or a user customization.
///
/// Compares content rather than testing file existence: `ensure_global()`
/// seeds every embedded prompt into the global prompts dir on first run, so
/// `code.md` being present on disk says nothing about whether the user
/// touched it. A prompt counts as [`UserFile`](crate::session::PromptSource::UserFile)
/// only when the text that actually shaped the session differs from the
/// compiled-in one, or has no compiled-in counterpart at all.
///
/// Reads the highest-precedence on-disk copy, mirroring the last-writer-wins
/// order in [`load()`]: `.zerostack/prompts/`, then the project's
/// `data/prompts/`, then the global dir. A lower-precedence copy being edited
/// is irrelevant when a higher-precedence one shadows it.
pub fn source_of(name: &str) -> crate::session::PromptSource {
    use crate::session::PromptSource;

    let file_name = format!("{name}.md");
    let effective = [zerostack_dir(), PathBuf::from("data/prompts"), global_dir()]
        .into_iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(&file_name)).ok());

    match effective {
        // Nothing on disk, so `load()` served the embedded copy. A name with
        // no embedded copy either cannot reach here: callers resolve against
        // `load()`'s map first.
        None => PromptSource::BuiltIn,
        // On disk but byte-identical to the embedded default: a seeded copy,
        // not a customization. Also covers a name with no embedded
        // counterpart, which can only have come from a user file.
        Some(content) => {
            let embedded = EMBEDDED
                .get_file(&file_name)
                .and_then(|f| f.contents_utf8());
            if embedded == Some(content.as_str()) {
                PromptSource::BuiltIn
            } else {
                PromptSource::UserFile
            }
        }
    }
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

    #[test]
    fn test_source_of_built_in_prompt() {
        let _td = TestDir::new();
        let prompts = load();
        assert!(prompts.contains_key("code"));
        assert_eq!(source_of("code"), crate::session::PromptSource::BuiltIn);
    }

    #[test]
    fn test_source_of_user_file_shadowing_a_built_in_name() {
        let _td = TestDir::new();
        let zs_dir = zerostack_dir();
        write_prompt(&zs_dir, "code", "from .zerostack/");

        let prompts = load();
        assert_eq!(prompts["code"], "from .zerostack/");
        assert_eq!(source_of("code"), crate::session::PromptSource::UserFile);
    }

    #[test]
    fn test_source_of_user_only_prompt() {
        let _td = TestDir::new();
        let dir = zerostack_dir();
        write_prompt(&dir, "myproject", "# My Project Prompt");

        let prompts = load();
        assert!(prompts.contains_key("myproject"));
        assert_eq!(
            source_of("myproject"),
            crate::session::PromptSource::UserFile
        );
    }

    /// The case a file-existence check gets wrong: every real run calls
    /// `ensure_global()`, which writes all embedded prompts to the global
    /// dir, so `code.md` exists on disk without the user having touched it.
    #[test]
    fn test_source_of_built_in_survives_ensure_global() {
        let _td = TestDir::new();
        ensure_global().unwrap();
        assert!(
            global_dir().join("code.md").exists(),
            "ensure_global should have seeded the global prompts dir"
        );

        assert_eq!(source_of("code"), crate::session::PromptSource::BuiltIn);
    }

    #[test]
    fn test_source_of_edited_global_copy() {
        let _td = TestDir::new();
        ensure_global().unwrap();
        write_prompt(&global_dir(), "code", "edited in place");

        let prompts = load();
        assert_eq!(prompts["code"], "edited in place");
        assert_eq!(source_of("code"), crate::session::PromptSource::UserFile);
    }

    /// Only the copy that actually shaped the session counts: an edited
    /// global copy is shadowed by a `.zerostack/` one holding the stock text.
    #[test]
    fn test_source_of_reads_highest_precedence_copy() {
        let _td = TestDir::new();
        let built_in = load()["code"].clone();
        write_prompt(&global_dir(), "code", "edited in place");
        write_prompt(&zerostack_dir(), "code", &built_in);

        let prompts = load();
        assert_eq!(prompts["code"], built_in);
        assert_eq!(source_of("code"), crate::session::PromptSource::BuiltIn);
    }
}
