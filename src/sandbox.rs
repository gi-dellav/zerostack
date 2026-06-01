use std::sync::OnceLock;

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Sandbox {
    enabled: bool,
    backend: String,
    shell: String,
}

static BWRAP_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn bwrap_exists() -> bool {
    *BWRAP_AVAILABLE.get_or_init(|| which_cmd("bwrap"))
}

static ZEROBOX_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn zerobox_exists() -> bool {
    *ZEROBOX_AVAILABLE.get_or_init(|| which_cmd("zerobox"))
}

fn which_cmd(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl Sandbox {
    pub fn new(enabled: bool, backend: &str) -> Self {
        Sandbox {
            enabled,
            backend: backend.to_string(),
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
            cmd.kill_on_drop(true);
            return cmd;
        }

        let cwd = std::env::current_dir().unwrap_or_default();

        if self.backend == "zerobox" {
            if !zerobox_exists() {
                tracing::warn!("sandbox: zerobox not found, running unsandboxed");
                let mut cmd = Command::new(&self.shell);
                cmd.arg("-c").arg(command);
                cmd.kill_on_drop(true);
                return cmd;
            }
            let mut cmd = Command::new("zerobox");
            cmd.arg("--allow-write");
            cmd.arg(cwd.as_os_str());
            cmd.arg("--");
            cmd.arg(&self.shell);
            cmd.arg("-c");
            cmd.arg(command);
            cmd.kill_on_drop(true);
            return cmd;
        }

        if !bwrap_exists() {
            tracing::warn!("sandbox: bwrap not found, running unsandboxed");
            let mut cmd = Command::new(&self.shell);
            cmd.arg("-c").arg(command);
            cmd.kill_on_drop(true);
            return cmd;
        }

        let mut cmd = Command::new("bwrap");
        cmd.arg("--clearenv");
        for (k, v) in essential_env() {
            cmd.arg("--setenv").arg(k).arg(v);
        }
        cmd.args(["--ro-bind", "/", "/", "--bind"]);
        cmd.arg(cwd.as_os_str());
        cmd.arg(cwd.as_os_str());
        cmd.args([
            "--ro-bind",
            "/sys",
            "/sys",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--ro-bind",
            "/etc/resolv.conf",
            "/etc/resolv.conf",
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-cgroup",
            "--die-with-parent",
            &self.shell,
            "-c",
            command,
        ]);
        cmd.kill_on_drop(true);
        cmd
    }
}

fn essential_env() -> Vec<(&'static str, String)> {
    let preserve = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
        "SSH_ASKPASS",
        "GIT_ASKPASS",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "EDITOR",
        "VISUAL",
        "LD_LIBRARY_PATH",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
        "GOROOT",
        "VIRTUAL_ENV",
        "JAVA_HOME",
        "NODE_PATH",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "COLORTERM",
        "NO_COLOR",
    ];
    let mut vars = Vec::with_capacity(preserve.len());
    for name in &preserve {
        if let Ok(val) = std::env::var(name) {
            vars.push((*name, val));
        }
    }
    vars
}
