use std::path::PathBuf;

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Sandbox {
    enabled: bool,
    backend: String,
    cwd: PathBuf,
    shell: String,
}

impl Sandbox {
    pub fn new(enabled: bool, backend: &str) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        Sandbox {
            enabled,
            backend: backend.to_string(),
            cwd,
            shell: "bash".to_string(),
        }
    }

    pub fn with_shell(mut self, shell: &str) -> Self {
        if !shell.is_empty() {
            self.shell = shell.to_string();
        }
        self
    }

    pub fn wrap_command(&self, command: &str) -> Command {
        if !self.enabled {
            let mut cmd = Command::new(&self.shell);
            cmd.arg("-c").arg(command);
            return cmd;
        }

        if self.backend == "zerobox" {
            let mut cmd = Command::new("zerobox");
            cmd.arg("--allow-write");
            cmd.arg(self.cwd.as_os_str());
            cmd.arg("--");
            cmd.arg(&self.shell);
            cmd.arg("-c");
            cmd.arg(command);
            return cmd;
        }

        let mut cmd = Command::new("bwrap");
        cmd.args(["--ro-bind", "/", "/", "--bind"]);
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
            &self.shell,
            "-c",
            command,
        ]);
        cmd
    }
}
