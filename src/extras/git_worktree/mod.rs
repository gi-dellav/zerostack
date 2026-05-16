use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub branch: String,
    pub worktree_path: PathBuf,
    pub main_repo_path: PathBuf,
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

    let worktree_path: PathBuf = Path::new(&git_dir).canonicalize().ok()?;

    if common_dir == git_dir {
        return None;
    }

    let main_repo_path: PathBuf = Path::new(&common_dir).parent().map(|p| p.to_path_buf())?;
    let main_repo_path = main_repo_path.canonicalize().ok()?;

    let branch = current_branch().unwrap_or_default();

    Some(WorktreeInfo {
        branch,
        worktree_path: worktree_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(worktree_path),
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

pub fn create(name: &str) -> Result<(PathBuf, WorktreeInfo), String> {
    let target = format!("../{}", name);

    let output = Command::new("git")
        .args(["worktree", "add", "-b", name, &target])
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr.trim()));
    }

    let wt_path = PathBuf::from(&target)
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
