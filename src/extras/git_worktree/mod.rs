// ponytail: all git invocations use synchronous std::process::Command, which
// blocks the tokio event loop. Under the default current_thread runtime this
// freezes the TUI during worktree merges. Migrate to tokio::process::Command
// and async functions if merge latency becomes noticeable.
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// RAII guard that restores the original working directory on drop. Ensures
/// the process CWD is restored even on panic. The `set_current_dir` approach
/// is process-global — under the `multithread` feature, other threads' CWD
/// reads are affected. // ponytail: use `git -C` everywhere if multithread matters.
struct ChdirGuard {
    orig_dir: PathBuf,
}

impl ChdirGuard {
    fn new(target: &Path) -> Result<Self, String> {
        let orig_dir = std::env::current_dir().map_err(|e| format!("current_dir: {}", e))?;
        std::env::set_current_dir(target)
            .map_err(|e| format!("cd to {}: {}", target.display(), e))?;
        Ok(ChdirGuard { orig_dir })
    }
}

impl Drop for ChdirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.orig_dir);
    }
}

#[derive(Debug, Clone)]
pub enum DeferredWorktreeAction {
    Merge {
        branch: String,
        target: String,
        main_path: String,
        wt_path: String,
    },
    Exit {
        main_path: String,
    },
}

impl fmt::Display for DeferredWorktreeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Merge { branch, target, .. } => {
                write!(f, "deferred worktree merge: {} -> {}", branch, target)
            }
            Self::Exit { main_path, .. } => {
                write!(f, "deferred worktree exit: back to {}", main_path)
            }
        }
    }
}

impl std::error::Error for DeferredWorktreeAction {}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub branch: String,
    pub worktree_path: PathBuf,
    pub main_repo_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Success,
    Conflicts(Vec<String>),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct MergeState {
    pub info: WorktreeInfo,
    pub original_branch: String,
    pub orig_dir: PathBuf,
    pub stashed: bool,
}

pub fn detect() -> Option<WorktreeInfo> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let common_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if common_dir == git_dir {
        return None;
    }

    // The git dir in a worktree looks like <main>/.git/worktrees/<name>
    // The actual working tree is stored in the `gitdir` file inside that directory.
    let git_dir_path = Path::new(&git_dir);
    let gitdir_file = git_dir_path.join("gitdir");

    let worktree_path = if gitdir_file.exists() {
        // Read the actual worktree path from the gitdir file
        std::fs::read_to_string(&gitdir_file)
            .ok()
            .and_then(|s| {
                let trimmed = s.trim();
                Path::new(trimmed).parent().map(|p| p.to_path_buf())
            })
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_else(|| {
                // Fallback: use canonicalized git-dir path
                git_dir_path
                    .canonicalize()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| git_dir_path.to_path_buf())
            })
    } else {
        // Simpler worktree structure: git-dir is at .git, parent is worktree root
        git_dir_path
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| git_dir_path.to_path_buf())
    };

    let main_repo_path = {
        let parent = Path::new(&common_dir).parent()?;
        parent
            .canonicalize()
            .ok()
            .unwrap_or_else(|| parent.to_path_buf())
    };

    let branch = current_branch().unwrap_or_default();

    Some(WorktreeInfo {
        branch,
        worktree_path,
        main_repo_path,
    })
}

pub fn current_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch == "HEAD" { None } else { Some(branch) }
}

pub fn default_branch(repo_path: &Path) -> Option<String> {
    for name in &["main", "master"] {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["rev-parse", "--verify", name])
            .output()
            .ok();
        if let Some(out) = output
            && out.status.success()
        {
            return Some(name.to_string());
        }
    }
    None
}

pub fn create(name: &str, base_dir: Option<&Path>) -> Result<(PathBuf, WorktreeInfo), String> {
    let target = match base_dir {
        Some(dir) => dir.join(name),
        None => PathBuf::from(format!("../{}", name)),
    };

    let output = Command::new("git")
        .args(["worktree", "add", "-b", name])
        .arg(&target)
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr.trim()));
    }

    let wt_path = target
        .canonicalize()
        .map_err(|e| format!("failed to resolve worktree path: {}", e))?;

    let main_repo =
        std::env::current_dir().map_err(|e| format!("failed to get current dir: {}", e))?;

    Ok((
        wt_path.clone(),
        WorktreeInfo {
            branch: name.to_string(),
            worktree_path: wt_path,
            main_repo_path: main_repo,
        },
    ))
}

pub fn repo_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Phase 1: Change to main repo, stash, fetch, checkout target, pull, merge.
/// On Success or Conflicts, current directory is left in the main repo (on target).
/// On Error, the function cleans up and restores the original directory.
pub fn try_merge(info: &WorktreeInfo, target: &str) -> (MergeState, MergeOutcome) {
    let orig_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                MergeState {
                    info: info.clone(),
                    original_branch: String::new(),
                    stashed: false,
                    orig_dir: PathBuf::new(),
                },
                MergeOutcome::Error(format!("current_dir: {}", e)),
            );
        }
    };

    if let Err(e) = std::env::set_current_dir(&info.main_repo_path) {
        return (
            MergeState {
                info: info.clone(),
                original_branch: String::new(),
                stashed: false,
                orig_dir,
            },
            MergeOutcome::Error(format!("cd to {}: {}", info.main_repo_path.display(), e)),
        );
    }

    let original_branch = current_branch().unwrap_or_default();
    let stashed = has_uncommitted_changes() && run_git(["stash", "--include-untracked"]).is_ok();

    // Helper to clean up on early-stage errors: pop stash, restore dir
    let cleanup_early = |state: &mut MergeState, err: String| -> MergeOutcome {
        if state.stashed
            && let Err(e) = run_git(["stash", "pop"])
        {
            tracing::error!(
                branch = %state.info.branch,
                error = %e,
                "worktree merge: failed to restore stash during early cleanup; \
                 stashed changes may be lost; try `git stash pop` manually"
            );
        }
        let _ = std::env::set_current_dir(&state.orig_dir);
        MergeOutcome::Error(err)
    };

    let mut working_state = MergeState {
        info: info.clone(),
        original_branch: original_branch.clone(),
        stashed,
        orig_dir: orig_dir.clone(),
    };

    if let Err(e) = run_git(["fetch", "--all"]) {
        let outcome = cleanup_early(&mut working_state, format!("fetch failed: {}", e));
        return (working_state, outcome);
    }

    if let Err(e) = run_git(["checkout", target]) {
        let outcome = cleanup_early(&mut working_state, format!("checkout failed: {}", e));
        return (working_state, outcome);
    }

    if let Err(e) = run_git(["pull", "--no-edit"]) {
        let _ = run_git_quiet(["checkout", &original_branch]);
        let outcome = cleanup_early(&mut working_state, format!("pull failed: {}", e));
        return (working_state, outcome);
    }

    match run_git(["merge", "--squash", &info.branch]) {
        Ok(_) => match run_git(["commit", "--no-edit"]) {
            Ok(_) => (working_state, MergeOutcome::Success),
            Err(e) if e.contains("nothing to commit") => (working_state, MergeOutcome::Success),
            Err(e) => {
                let _ = run_git_quiet(["reset", "--merge"]);
                let _ = run_git_quiet(["checkout", &original_branch]);
                let outcome = cleanup_early(
                    &mut working_state,
                    format!("commit after squash failed: {}", e),
                );
                (working_state, outcome)
            }
        },
        Err(_) if has_merge_conflict() => {
            let files = conflicted_files();
            (working_state, MergeOutcome::Conflicts(files))
        }
        Err(e) => {
            tracing::error!(
                branch = %info.branch,
                target = %target,
                error = %e,
                "worktree merge: merge failed, aborting"
            );
            let _ = run_git_quiet(["merge", "--abort"]);
            let _ = run_git_quiet(["checkout", &original_branch]);
            let outcome = cleanup_early(&mut working_state, format!("merge failed: {}", e));
            (working_state, outcome)
        }
    }
}

/// Phase 2: After a successful merge (or after conflicts are resolved),
/// push, delete the worktree, delete the branch, restore original dir.
/// If `force` is true, use --force on worktree remove and branch delete.
pub fn complete_merge(state: &MergeState) -> Result<(), String> {
    complete_merge_with_force(state, false)
}

pub fn complete_merge_force(state: &MergeState) -> Result<(), String> {
    complete_merge_with_force(state, true)
}

fn complete_merge_with_force(state: &MergeState, force: bool) -> Result<(), String> {
    let current = std::env::current_dir().map_err(|e| e.to_string())?;
    let _ = std::env::set_current_dir(&state.info.main_repo_path);

    let result = (|| {
        if force {
            run_git([
                "worktree",
                "remove",
                "--force",
                &state.info.worktree_path.to_string_lossy(),
            ])?;
            run_git(["branch", "-D", &state.info.branch])?;
        } else {
            run_git([
                "worktree",
                "remove",
                &state.info.worktree_path.to_string_lossy(),
            ])?;
            run_git(["branch", "-d", &state.info.branch])?;
        }
        Ok::<(), String>(())
    })();

    if let Err(e) = &result {
        tracing::error!(
            branch = %state.info.branch,
            error = %e,
            "worktree complete_merge: cleanup failed"
        );
        let _ = std::env::set_current_dir(&current);
    } else {
        if state.stashed
            && let Err(e) = run_git(["stash", "pop"])
        {
            tracing::error!(
                branch = %state.info.branch,
                error = %e,
                "worktree complete_merge: failed to pop stash; \
                 changes may be lost; try `git stash pop` manually"
            );
            let _ = std::env::set_current_dir(&state.orig_dir);
            return Err(format!(
                "merge succeeded but stash pop failed: {}. \
                     Your changes are in the stash; run `git stash pop` manually.",
                e
            ));
        }
        let _ = std::env::set_current_dir(&state.orig_dir);
    }

    result
}

/// Best-effort cleanup of a worktree after a merge. Safe to call even if the
/// worktree or branch has already been removed (idempotent).
pub fn cleanup_worktree(wt_path: &str, branch: &str, main_repo_path: &str, force: bool) {
    // ChdirGuard ensures the original CWD is restored on drop, even on panic.
    let _guard = match ChdirGuard::new(Path::new(main_repo_path)) {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("cleanup_worktree: {}", e);
            return;
        }
    };
    let remove_output = if force {
        Command::new("git")
            .args(["worktree", "remove", "--force", wt_path])
            .output()
    } else {
        Command::new("git")
            .args(["worktree", "remove", wt_path])
            .output()
    };
    if let Ok(out) = &remove_output {
        if out.status.success() {
            tracing::info!(branch, wt_path, "cleanup_worktree: removed worktree");
        } else {
            tracing::debug!(
                branch,
                wt_path,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "cleanup_worktree: git worktree remove (already gone or failed)"
            );
        }
    }

    let branch_flag = if force { "-D" } else { "-d" };
    let branch_output = Command::new("git")
        .args(["branch", branch_flag, branch])
        .output();
    if let Ok(out) = &branch_output {
        if out.status.success() {
            tracing::info!(branch, "cleanup_worktree: deleted branch");
        } else {
            tracing::debug!(
                branch,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "cleanup_worktree: git branch delete (already gone or failed)"
            );
        }
    }
}

/// Cancel an in-progress merge: abort, restore original branch, pop stash, restore dir.
pub fn cancel_merge(state: &MergeState) -> Result<(), String> {
    // ChdirGuard restores CWD on drop; override orig_dir to state.orig_dir
    // so it restores to the pre-merge directory, not the current one.
    let mut guard = ChdirGuard::new(&state.info.main_repo_path)?;
    guard.orig_dir = state.orig_dir.clone();

    if has_merge_conflict() {
        run_git_quiet_logged(["merge", "--abort"], "cancel_merge: merge --abort")?
    }
    if !state.original_branch.is_empty() {
        let _ = run_git_quiet_logged(
            ["checkout", &state.original_branch],
            "cancel_merge: checkout original",
        );
    }
    if state.stashed
        && let Err(e) = run_git_quiet_logged(["stash", "pop"], "cancel_merge: stash pop")
    {
        tracing::error!(
            branch = %state.info.branch,
            error = %e,
            "cancel_merge: failed to pop stash; try `git stash pop` manually"
        );
    }

    Ok(())
}

/// Check if there is an active merge conflict in the current directory.
pub fn has_merge_conflict() -> bool {
    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok();
    if let Some(out) = output
        && out.status.success()
    {
        let git_dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let merge_head = Path::new(&git_dir).join("MERGE_HEAD");
        if merge_head.exists() {
            return true;
        }
    }
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();
    match output {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        _ => false,
    }
}

/// List files with merge conflicts in the current directory.
pub fn conflicted_files() -> Vec<String> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .trim()
            .lines()
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

// --- Private helpers ---

fn run_git<const N: usize>(args: [&str; N]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let err = format!("git {} failed: {}", args.join(" "), stderr.trim());
        tracing::debug!("{}", err);
        return Err(err);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_quiet<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!("git {} failed silently: {}", args.join(" "), stderr.trim());
        None
    }
}

fn run_git_quiet_logged<const N: usize>(args: [&str; N], context: &str) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let err = format!(
            "{}: git {} failed: {}",
            context,
            args.join(" "),
            stderr.trim()
        );
        tracing::warn!("{}", err);
        return Err(err);
    }
    Ok(())
}

fn has_uncommitted_changes() -> bool {
    let output = Command::new("git").args(["status", "--porcelain"]).output();
    match output {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        _ => false,
    }
}

pub fn worktree_has_uncommitted(wt_path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["status", "--porcelain"])
        .output();
    match output {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        _ => false,
    }
}

pub fn worktree_auto_commit_all(wt_path: &Path) -> Result<(), String> {
    // Use `-u` (update tracked files only) instead of `-A` (all including
    // untracked) to avoid auto-committing new files that may contain secrets.
    let output = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["add", "-u"])
        .output()
        .map_err(|e| format!("git add failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git add -u failed: {}", stderr.trim()));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["commit", "-m", "auto-commit: save changes before merge"])
        .output()
        .map_err(|e| format!("git commit failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git commit failed: {}", stderr.trim()));
    }
    Ok(())
}
