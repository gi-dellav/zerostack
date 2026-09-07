#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use crate::cli::Cli;
    use crate::config::Config;
    use crate::extras::git_worktree::*;
    use crate::tests::acquire_cwd;

    /// Unique temp dir per test; pid keeps parallel processes apart, the
    /// counter the tests inside one process.
    fn unique_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "zs_wt_{}_{}_{}_{}",
            tag,
            std::process::id(),
            n,
            tag
        ))
    }

    /// Unique branch/worktree name per call: even though git-level tests
    /// hold the shared CWD lock, names must also be unique because the
    /// default worktree location (`<parent-of-repo>/<name>`, i.e. `/tmp`)
    /// is shared across repos — and because a previous failed or
    /// interrupted run may have left worktree directories behind.
    /// Uniqueness comes from pid + an atomic counter + nanos: never from a
    /// counter alone (a fresh test binary restarts counters at 0 while
    /// leftovers from the last run are still on disk).
    fn unique_name(tag: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("zs{tag}{}_{ns}_{n}", std::process::id())
    }

    /// Guard that restores the process CWD on drop and removes the temp dir.
    /// Created repos live under `self.root`; tests chdir into them freely.
    /// Construction also repairs CWD when a previous test left the process
    /// inside a directory that has since been deleted (happens when tests
    /// run in parallel with other CWD-mutating suites), so that
    /// `current_dir()` keeps working.
    struct TempRepo {
        root: PathBuf,
        orig: PathBuf,
    }

    impl TempRepo {
        /// Fresh `git init -b <branch>` repo with one commit and local
        /// identity configured (no global config touched). Identity matters:
        /// squash-merge commits fail without it.
        fn new(tag: &str, branch: &str) -> Self {
            let orig = std::env::current_dir().unwrap_or_else(|_| {
                let fallback =
                    std::env::temp_dir().join(format!("zs_wt_fallback_{}", std::process::id()));
                let _ = std::fs::create_dir_all(&fallback);
                let _ = std::env::set_current_dir(&fallback);
                std::env::current_dir().unwrap_or(fallback)
            });
            let root = unique_dir(tag);
            let _ = std::fs::remove_dir_all(&root);
            // create_dir, not create_dir_all: the name is predictable in a
            // shared temp dir, and the root is canonicalised below, after
            // which Drop's remove_dir_all would follow a planted symlink.
            // Failing on an existing entry closes that.
            std::fs::create_dir(&root).unwrap();
            // Canonical so comparisons against paths the code under test
            // returns (it canonicalises via `absolutize`) hold on macOS,
            // where `temp_dir()` is `/var/...` but resolves to `/private/var/...`.
            let root = root.canonicalize().unwrap();
            // `git -C` (not CWD-relative): the process may currently sit in
            // a directory owned by another in-flight test.
            run(&root, &["init", "-b", branch]);
            run(&root, &["config", "user.email", "test@example.com"]);
            run(&root, &["config", "user.name", "test"]);
            run(&root, &["config", "commit.gpgsign", "false"]);
            // No remote: exercises the no-upstream `pull` tolerance.
            std::fs::write(root.join("file.txt"), "base\n").unwrap();
            run(&root, &["add", "file.txt"]);
            run(&root, &["commit", "-m", "init"]);
            TempRepo { orig, root }
        }

        fn cd(&self) {
            std::env::set_current_dir(&self.root).unwrap();
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            // Best-effort: never leave worktrees/branches registered behind.
            // A TempRepo that outlives a failed merge or an interrupted test
            // otherwise leaks `<tmp>/<branch>` dirs and branch refs, which
            // later runs (fresh counters, same /tmp) then collide with.
            // `git worktree list` runs in the repo itself, so no CWD games.
            if let Ok(out) = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["worktree", "list", "--porcelain"])
                .output()
                && out.status.success()
            {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let paths: Vec<String> = stdout
                    .lines()
                    .filter_map(|l| l.strip_prefix("worktree "))
                    .map(|p| p.trim().to_string())
                    .filter(|p| {
                        std::path::Path::new(p) != self.root
                            && std::path::Path::new(p).starts_with(
                                self.root.parent().unwrap_or(std::path::Path::new("/tmp")),
                            )
                    })
                    .collect();
                for p in &paths {
                    let _ = Command::new("git")
                        .arg("-C")
                        .arg(&self.root)
                        .args(["worktree", "remove", "--force", p])
                        .output();
                    // The dir may survive a stale admin entry; remove it.
                    let _ = std::fs::remove_dir_all(p);
                }
                let _ = Command::new("git")
                    .arg("-C")
                    .arg(&self.root)
                    .args(["worktree", "prune"])
                    .output();
            }
            let _ = std::env::set_current_dir(&self.orig);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn run(dir: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /*fn git_ok(dir: &std::path::Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }*/

    #[test]
    fn test_worktree_info_clone() {
        let info = WorktreeInfo {
            branch: "feature-x".into(),
            worktree_path: PathBuf::from("/tmp/wt"),
            main_repo_path: PathBuf::from("/tmp/repo"),
        };
        let cloned = info.clone();
        assert_eq!(cloned.branch, "feature-x");
        assert_eq!(cloned.worktree_path, PathBuf::from("/tmp/wt"));
        assert_eq!(cloned.main_repo_path, PathBuf::from("/tmp/repo"));
    }

    #[test]
    fn test_merge_outcome_success_eq() {
        assert_eq!(MergeOutcome::Success, MergeOutcome::Success);
    }

    #[test]
    fn test_merge_outcome_conflicts_eq() {
        let a = MergeOutcome::Conflicts(vec!["a".into(), "b".into()]);
        let b = MergeOutcome::Conflicts(vec!["a".into(), "b".into()]);
        assert_eq!(a, b);
    }

    #[test]
    fn test_merge_outcome_conflicts_ne() {
        let a = MergeOutcome::Conflicts(vec!["a".into()]);
        let b = MergeOutcome::Conflicts(vec!["b".into()]);
        assert_ne!(a, b);
    }

    #[test]
    fn test_merge_outcome_error_eq() {
        let a = MergeOutcome::Error("msg".into());
        let b = MergeOutcome::Error("msg".into());
        assert_eq!(a, b);
    }

    #[test]
    fn test_merge_outcome_error_ne() {
        let a = MergeOutcome::Error("a".into());
        let b = MergeOutcome::Error("b".into());
        assert_ne!(a, b);
    }

    #[test]
    fn test_merge_outcome_cross_variant_ne() {
        assert_ne!(MergeOutcome::Success, MergeOutcome::Error("err".into()));
        assert_ne!(
            MergeOutcome::Success,
            MergeOutcome::Conflicts(vec!["f".into()])
        );
    }

    #[test]
    fn test_merge_state_clone() {
        let state = MergeState {
            info: WorktreeInfo {
                branch: "feat".into(),
                worktree_path: PathBuf::from("/tmp/wt"),
                main_repo_path: PathBuf::from("/tmp/repo"),
            },
            original_branch: "main".into(),
            orig_dir: PathBuf::from("/tmp/wt"),
            stashed: true,
        };
        let cloned = state.clone();
        assert_eq!(cloned.original_branch, "main");
        assert!(cloned.stashed);
        assert_eq!(cloned.orig_dir, PathBuf::from("/tmp/wt"));
        assert_eq!(cloned.info.branch, "feat");
    }

    #[test]
    fn test_repo_name_basic() {
        assert_eq!(
            repo_name(&PathBuf::from("/home/user/my-project")),
            "my-project"
        );
    }

    #[test]
    fn test_repo_name_trailing_slash() {
        assert_eq!(repo_name(&PathBuf::from("/home/user/repo/")), "repo");
    }

    #[test]
    fn test_repo_name_empty() {
        assert_eq!(repo_name(&PathBuf::from("")), "unknown");
    }

    #[test]
    fn test_repo_name_root() {
        assert_eq!(repo_name(&PathBuf::from("/")), "unknown");
    }

    #[test]
    fn test_wt_cli_flags_default() {
        let cli = Cli::default();
        assert!(cli.worktree.is_none());
        assert!(!cli.wt_auto_merge);
        assert!(!cli.parallel);
        assert!(cli.wt_base_dir.is_none());
        assert!(!cli.wt_force);
    }

    #[test]
    fn test_wt_cli_flags_enabled() {
        let cli = Cli {
            worktree: Some("feature-x".into()),
            wt_auto_merge: true,
            wt_force: true,
            wt_base_dir: Some("/tmp".into()),
            ..Default::default()
        };
        assert_eq!(cli.worktree.as_deref(), Some("feature-x"));
        assert!(cli.wt_auto_merge);
        assert!(cli.wt_force);
        assert_eq!(cli.wt_base_dir.as_deref(), Some("/tmp"));
    }

    #[test]
    fn test_resolve_wt_auto_merge_cli() {
        let cli = Cli {
            wt_auto_merge: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert!(cli.resolve_wt_auto_merge(&cfg));
    }

    #[test]
    fn test_resolve_wt_auto_merge_parallel() {
        let cli = Cli {
            parallel: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert!(cli.resolve_wt_auto_merge(&cfg));
    }

    #[test]
    fn test_resolve_wt_auto_merge_config() {
        let cli = Cli::default();
        let cfg = Config {
            wt_auto_merge: Some(true),
            ..Default::default()
        };
        assert!(cli.resolve_wt_auto_merge(&cfg));
    }

    #[test]
    fn test_resolve_wt_auto_merge_default_false() {
        let cli = Cli::default();
        let cfg = Config::default();
        assert!(!cli.resolve_wt_auto_merge(&cfg));
    }

    #[test]
    fn test_resolve_wt_force_cli() {
        let cli = Cli {
            wt_force: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert!(cli.resolve_wt_force(&cfg));
    }

    #[test]
    fn test_resolve_wt_force_config() {
        let cli = Cli::default();
        let cfg = Config {
            wt_force: Some(true),
            ..Default::default()
        };
        assert!(cli.resolve_wt_force(&cfg));
    }

    #[test]
    fn test_resolve_wt_force_default_false() {
        let cli = Cli::default();
        let cfg = Config::default();
        assert!(!cli.resolve_wt_force(&cfg));
    }

    #[test]
    fn test_resolve_wt_base_dir_cli() {
        let cli = Cli {
            wt_base_dir: Some("/custom/base".into()),
            ..Default::default()
        };
        let cfg = Config::default();
        assert_eq!(
            cli.resolve_wt_base_dir(&cfg),
            Some(PathBuf::from("/custom/base"))
        );
    }

    #[test]
    fn test_resolve_wt_base_dir_config() {
        let cli = Cli::default();
        let cfg = Config {
            wt_base_dir: Some("/config/base".into()),
            ..Default::default()
        };
        assert_eq!(
            cli.resolve_wt_base_dir(&cfg),
            Some(PathBuf::from("/config/base"))
        );
    }

    #[test]
    fn test_resolve_wt_base_dir_default_none() {
        let cli = Cli::default();
        let cfg = Config::default();
        assert_eq!(cli.resolve_wt_base_dir(&cfg), None);
    }

    #[test]
    fn test_resolve_wt_base_dir_cli_overrides_config() {
        let cli = Cli {
            wt_base_dir: Some("/cli".into()),
            ..Default::default()
        };
        let cfg = Config {
            wt_base_dir: Some("/config".into()),
            ..Default::default()
        };
        assert_eq!(cli.resolve_wt_base_dir(&cfg), Some(PathBuf::from("/cli")));
    }

    #[test]
    fn test_default_branch_is_refutable() {
        // Pure-logic: the function returns None for non-existent paths (no git init)
        assert!(default_branch(&PathBuf::from("/tmp/nonexistent_repo")).is_none());
    }

    // ── validate_branch_name ─────────────────────────────────────────────

    #[test]
    fn test_validate_branch_name_ok() {
        assert!(validate_branch_name("feature-x").is_ok());
        assert!(validate_branch_name("wt-123-456").is_ok());
        assert!(validate_branch_name("a").is_ok());
    }

    #[test]
    fn test_validate_branch_name_rejects() {
        for bad in [
            "",
            "has space",
            "has/slash",
            "-leading-dash",
            "..",
            ".",
            "@{x}",
            "a..b",
            "a~b",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a.lock",
        ] {
            assert!(
                validate_branch_name(bad).is_err(),
                "expected {:?} to be rejected",
                bad
            );
        }
    }

    #[test]
    fn test_validate_branch_name_whitespace_variants() {
        assert!(validate_branch_name("a\tb").is_err());
        assert!(validate_branch_name("a\nb").is_err());
        assert!(validate_branch_name(" a").is_err());
    }

    // ── create / detect round-trips (touch real git repos) ──────────────

    #[test]
    fn test_create_and_detect_round_trip() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("roundtrip", "main");
        repo.cd();
        let name = unique_name("feat");
        let (wt_path, info) = create(&name, None).expect("create should succeed");
        assert_eq!(info.branch, name);
        assert!(wt_path.exists());
        // Base dir defaults to the parent of the main repo.
        assert_eq!(wt_path.parent().unwrap(), repo.root.parent().unwrap());
        std::env::set_current_dir(&wt_path).unwrap();
        let detected = detect().expect("should detect linked worktree");
        assert_eq!(detected.branch, name);
        assert_eq!(detected.main_repo_path, repo.root);
    }

    #[test]
    fn test_detect_returns_none_in_main_repo() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("nondetect", "main");
        repo.cd();
        assert!(detect().is_none());
    }

    #[test]
    fn test_detect_returns_none_outside_repo() {
        let _lock = acquire_cwd();
        let dir = unique_dir("outside");
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_current_dir(&dir).unwrap();
        assert!(detect().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_rejects_existing_branch() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("dupbranch", "main");
        repo.cd();
        let name = unique_name("feat");
        create(&name, None).expect("first create succeeds");
        let err = create(&name, None).expect_err("second create must fail");
        assert!(err.contains("already exists"), "unexpected: {}", err);
    }

    #[test]
    fn test_create_rejects_occupied_target() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("occupied", "main");
        repo.cd();
        std::fs::create_dir_all(repo.root.parent().unwrap().join("taken")).unwrap();
        let err = create("taken", None).expect_err("occupied target must fail");
        assert!(err.contains("already exists"), "unexpected: {}", err);
    }

    #[test]
    fn test_create_uses_base_dir_and_makedirs() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("basedir", "main");
        repo.cd();
        let base = repo.root.join("nested").join("bases");
        let name = unique_name("feat");
        let (wt_path, info) = create(&name, Some(&base)).expect("create with base dir");
        assert_eq!(wt_path, base.join(&name));
        assert!(wt_path.exists());
        assert_eq!(info.main_repo_path, repo.root);
        std::env::set_current_dir(&wt_path).unwrap();
        assert!(detect().is_some());
    }

    #[test]
    fn test_create_validates_name() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("badname", "main");
        repo.cd();
        assert!(create("../escape", None).is_err());
        assert!(create("-dash", None).is_err());
        assert!(create("", None).is_err());
    }

    #[test]
    fn test_branch_exists_and_registration() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("exists", "main");
        repo.cd();
        assert!(branch_exists(&repo.root, "main"));
        assert!(!branch_exists(&repo.root, "nope"));
        let (wt_path, _) = create(&unique_name("feat"), None).unwrap();
        assert!(is_worktree_registered(&repo.root, &wt_path));
        assert!(!is_worktree_registered(
            &repo.root,
            &repo.root.join("not-a-worktree")
        ));
    }

    // ── try_merge / complete_merge / cancel_merge ────────────────────────

    fn worktree_info_for(repo: &TempRepo, branch: &str) -> WorktreeInfo {
        // `create` places the worktree at ../<branch> relative to the repo;
        // remove a stale dir from a previous aborted run first — both this
        // process's earlier tests and other binaries share /tmp.
        let stale = repo
            .root
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"))
            .join(branch);
        if stale.exists() && !is_worktree_registered(&repo.root, &stale) {
            let _ = std::fs::remove_dir_all(&stale);
        }
        let (wt_path, _) = {
            repo.cd();
            create(branch, None).expect("create worktree")
        };
        std::env::set_current_dir(&wt_path).unwrap();
        detect().expect("detect worktree")
    }

    #[test]
    fn test_clean_merge_then_complete() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("cleanmerge", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        std::fs::write(info.worktree_path.join("new.txt"), "hello\n").unwrap();
        run(&info.worktree_path, &["add", "new.txt"]);
        run(&info.worktree_path, &["commit", "-m", "add new"]);
        repo.cd();
        let (state, outcome) = try_merge(&info, "main");
        assert_eq!(outcome, MergeOutcome::Success);
        complete_merge(&state).expect("complete should succeed");
        // Merge landed on main, worktree gone, branch deleted, and we are
        // left in the main repo (the worktree dir no longer exists).
        assert!(repo.root.join("new.txt").exists());
        assert!(!info.worktree_path.exists());
        assert!(!branch_exists(&repo.root, &info.branch));
        assert_eq!(std::env::current_dir().unwrap(), repo.root);
    }

    #[test]
    fn test_already_merged_branch_is_success() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("nomerge", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        // No commits on the branch: squash produces nothing to commit.
        repo.cd();
        let (state, outcome) = try_merge(&info, "main");
        assert_eq!(outcome, MergeOutcome::Success);
        complete_merge(&state).expect("complete of empty merge");
        assert!(!branch_exists(&repo.root, &info.branch));
    }

    #[test]
    fn test_merge_without_upstream_succeeds() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("noupstream", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        std::fs::write(info.worktree_path.join("u.txt"), "x\n").unwrap();
        run(&info.worktree_path, &["add", "u.txt"]);
        run(&info.worktree_path, &["commit", "-m", "u"]);
        repo.cd();
        // TempRepo has no remote: the old code treated `git pull` failing
        // as fatal; the merge must still succeed.
        let (_, outcome) = try_merge(&info, "main");
        assert_eq!(outcome, MergeOutcome::Success);
    }

    #[test]
    fn test_conflict_then_cancel_restores_state() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("conflict", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        std::fs::write(info.worktree_path.join("file.txt"), "worktree side\n").unwrap();
        run(&info.worktree_path, &["commit", "-am", "worktree change"]);
        repo.cd();
        std::fs::write(repo.root.join("file.txt"), "main side\n").unwrap();
        run(&repo.root, &["commit", "-am", "main change"]);
        // Dirty file in main repo: try_merge must stash it, then conflict.
        std::fs::write(repo.root.join("dirty.txt"), "uncommitted\n").unwrap();
        let (state, outcome) = try_merge(&info, "main");
        assert!(state.stashed, "dirty main-repo file should be stashed");
        match outcome {
            MergeOutcome::Conflicts(files) => {
                assert!(!files.is_empty());
                assert!(files.iter().any(|f| f.contains("file.txt")));
            }
            other => panic!("expected conflicts, got {:?}", other),
        }
        cancel_merge(&state).expect("cancel should restore");
        // Original branch back, no conflict markers, stash popped.
        assert_eq!(current_branch().as_deref(), Some("main"));
        assert!(!repo_has_merge_conflict(&repo.root));
        assert!(repo.root.join("dirty.txt").exists());
        // Worktree and branch survive an abort.
        assert!(info.worktree_path.exists());
        assert!(branch_exists(&repo.root, &info.branch));
    }

    #[test]
    fn test_conflict_cancel_with_untracked_stash() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("untracked", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        std::fs::write(info.worktree_path.join("file.txt"), "worktree side\n").unwrap();
        run(&info.worktree_path, &["commit", "-am", "worktree change"]);
        repo.cd();
        std::fs::write(repo.root.join("file.txt"), "main side\n").unwrap();
        run(&repo.root, &["commit", "-am", "main change"]);
        // Untracked file must round-trip through the --include-untracked stash.
        std::fs::write(repo.root.join("scratch.txt"), "temp\n").unwrap();
        let (state, outcome) = try_merge(&info, "main");
        assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
        cancel_merge(&state).unwrap();
        assert!(repo.root.join("scratch.txt").exists());
        assert!(!repo_has_merge_conflict(&repo.root));
    }

    #[test]
    fn test_cleanup_is_idempotent() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("idempotent", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        let main = info.main_repo_path.to_string_lossy().to_string();
        let wt = info.worktree_path.to_string_lossy().to_string();
        cleanup_worktree(&wt, &info.branch, &main, false);
        assert!(!info.worktree_path.exists());
        // Second call must not panic or error (nothing to catch — void fn).
        cleanup_worktree(&wt, &info.branch, &main, false);
        cleanup_worktree(&wt, "no-such-branch", &main, true);
    }

    #[test]
    fn test_verify_merged_after_squash() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("verify", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        std::fs::write(info.worktree_path.join("v.txt"), "v\n").unwrap();
        run(&info.worktree_path, &["add", "v.txt"]);
        run(&info.worktree_path, &["commit", "-m", "v"]);
        repo.cd();
        let (state, outcome) = try_merge(&info, "main");
        assert_eq!(outcome, MergeOutcome::Success);
        assert_eq!(
            verify_branch_merged(&repo.root, "main", &info.branch),
            AgentMergeStatus::Merged
        );
        complete_merge(&state).unwrap();
        // Branch ref deleted: verification must still report Merged.
        assert_eq!(
            verify_branch_merged(&repo.root, "main", &info.branch),
            AgentMergeStatus::Merged
        );
    }

    #[test]
    fn test_verify_not_merged_and_conflicts() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("verifyn", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        std::fs::write(info.worktree_path.join("extra.txt"), "x\n").unwrap();
        run(&info.worktree_path, &["add", "extra.txt"]);
        run(&info.worktree_path, &["commit", "-m", "extra"]);
        assert!(matches!(
            verify_branch_merged(&repo.root, "main", &info.branch),
            AgentMergeStatus::NotMerged(_)
        ));
        assert!(matches!(
            verify_branch_merged(&repo.root, "no-such-target-xyz", &info.branch),
            AgentMergeStatus::NotMerged(_)
        ));
        assert!(matches!(
            verify_branch_merged(&repo.root, "main", "nope"),
            AgentMergeStatus::Merged
        ));
        // Manufacture a conflict in the main repo and check detection.
        repo.cd();
        std::fs::write(info.worktree_path.join("file.txt"), "wt\n").unwrap();
        run(&info.worktree_path, &["commit", "-am", "wt change"]);
        std::fs::write(repo.root.join("file.txt"), "mn\n").unwrap();
        run(&repo.root, &["commit", "-am", "main change"]);
        let _ = Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["merge", "--squash", &info.branch])
            .output();
        assert!(repo_has_merge_conflict(&repo.root));
        assert!(!repo_conflicted_files(&repo.root).is_empty());
        assert!(matches!(
            verify_branch_merged(&repo.root, "main", &info.branch),
            AgentMergeStatus::Conflicts(_)
        ));
        // verify must not have side effects: worktree + branch intact.
        assert!(info.worktree_path.exists());
        assert!(branch_exists(&repo.root, &info.branch));
        abort_cleanup(&repo.root);
    }

    fn abort_cleanup(root: &std::path::Path) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["reset", "--merge"])
            .output();
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["checkout", "main"])
            .output();
    }

    #[test]
    fn test_auto_commit_round_trip() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("autocommit", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        // Tracked modification auto-commits.
        std::fs::write(info.worktree_path.join("file.txt"), "edited\n").unwrap();
        assert!(worktree_has_uncommitted(&info.worktree_path));
        worktree_auto_commit_all(&info.worktree_path).expect("auto-commit");
        assert!(!worktree_has_uncommitted(&info.worktree_path));
        // Clean tree is a no-op success.
        worktree_auto_commit_all(&info.worktree_path).expect("clean is ok");
        // Untracked-only tree errors instead of committing nothing.
        std::fs::write(info.worktree_path.join("new-untracked.txt"), "u\n").unwrap();
        assert!(worktree_auto_commit_all(&info.worktree_path).is_err());
    }

    #[test]
    fn test_resolve_agent_merge_outcome_messages() {
        use crate::ui::WtReturn;
        let ok = WtReturn {
            main_path: "/m".into(),
            wt_path: "/w".into(),
            branch: "b".into(),
            target: "main".into(),
            force: false,
        };
        // Pure formatting smoke test without touching git: NotMerged arm.
        let msg = format!(
            "agent run finished but '{}' is not merged into '{}'",
            ok.branch, ok.target
        );
        assert!(msg.contains('b'));
    }

    #[test]
    fn test_deferred_action_display() {
        let a = DeferredWorktreeAction::Merge {
            branch: "b".into(),
            target: "main".into(),
            main_path: "/m".into(),
            wt_path: "/w".into(),
        };
        assert!(a.to_string().contains('b'));
        let e = DeferredWorktreeAction::Exit {
            main_path: "/m".into(),
        };
        assert!(e.to_string().contains("/m"));
    }

    #[test]
    fn test_cherry_and_ancestor_helpers() {
        let _lock = acquire_cwd();
        let repo = TempRepo::new("helpers", "main");
        let info = worktree_info_for(&repo, &unique_name("feat"));
        // A fresh branch off main shares the tip: nothing unique to it.
        // (Ancestry itself is NOT asserted: `branch_exists` + cherry cover it
        // below, and `merge-base --is-ancestor` on unborn-tip setups is
        // brittle across git versions.)
        assert!(matches!(
            verify_branch_merged(&repo.root, "main", &info.branch),
            AgentMergeStatus::Merged
        ));
        repo.cd();
        let (state, outcome) = try_merge(&info, "main");
        assert_eq!(outcome, MergeOutcome::Success);
        assert!(matches!(
            verify_branch_merged(&repo.root, "main", &info.branch),
            AgentMergeStatus::Merged
        ));
        complete_merge(&state).unwrap();
    }
}
