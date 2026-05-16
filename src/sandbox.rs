use std::path::PathBuf;

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Sandbox {
    enabled: bool,
    cwd: PathBuf,
}

impl Sandbox {
    pub fn new(enabled: bool) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        Sandbox { enabled, cwd }
    }

    pub fn wrap_command(&self, command: &str) -> Command {
        if !self.enabled {
            let mut cmd = Command::new("bash");
            cmd.arg("-c").arg(command);
            return cmd;
        }

        let mut cmd = Command::new("bwrap");
        cmd.args([
            "--ro-bind",
            "/",
            "/",
            "--bind",
        ]);
        cmd.arg(self.cwd.as_os_str());
        cmd.arg(self.cwd.as_os_str());
        cmd.args([
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--unshare-all",
            "--die-with-parent",
            "bash",
            "-c",
            command,
        ]);
        cmd
    }
}
