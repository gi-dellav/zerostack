use std::collections::HashSet;
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Sandbox {
    enabled: bool,
    required: bool,
    backend: String,
    shell: String,
    // Test seams: `backend_available` replaces the cached PATH probe for the
    // backend binary (and `backend_installed_now` the fresh one) so tests
    // behave the same on hosts with and without bwrap, while `cache_dir` keeps
    // the bwrap arg builder away from the real user cache.
    backend_available: Option<bool>,
    backend_installed_now: Option<bool>,
    cache_dir: Option<std::path::PathBuf>,
    active_groups: Arc<Mutex<HashSet<u32>>>,
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
    // Search PATH directly rather than shelling out to `which`, which may not
    // exist on minimal images (Alpine, distroless).
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

pub(crate) struct ProcessGroupGuard {
    pid: Option<u32>,
    active_groups: Arc<Mutex<HashSet<u32>>>,
}

impl ProcessGroupGuard {
    pub(crate) fn new(pid: Option<u32>, active_groups: Arc<Mutex<HashSet<u32>>>) -> Self {
        if let Some(pid) = pid {
            active_groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(pid);
        }
        Self { pid, active_groups }
    }

    pub(crate) fn disarm(&mut self) {
        if let Some(pid) = self.pid.take() {
            self.active_groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&pid);
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            self.active_groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&pid);
            kill_process_group(pid);
        }
    }
}

impl Sandbox {
    pub fn new(enabled: bool, backend: &str) -> Self {
        Sandbox {
            enabled,
            required: false,
            backend: backend.to_string(),
            shell: "bash".to_string(),
            backend_available: None,
            backend_installed_now: None,
            cache_dir: None,
            active_groups: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Returns true if the sandbox is enabled and the backend binary is
    /// actually available. When false, commands run unsandboxed (or are
    /// refused when the sandbox is required), and the UI should surface it.
    pub fn is_effectively_sandboxed(&self) -> bool {
        self.enabled && self.backend_available()
    }

    pub fn with_shell(mut self, shell: &str) -> Self {
        if !shell.is_empty() {
            self.shell = shell.to_string();
        }
        self
    }

    /// When required, bash commands are refused instead of running
    /// unsandboxed if the backend binary is missing.
    pub fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    #[cfg(test)]
    pub fn with_backend_available(mut self, available: bool) -> Self {
        self.backend_available = Some(available);
        self
    }

    /// Overrides the fresh probe only, so a test can model a backend that was
    /// installed after the cached probe ran.
    #[cfg(test)]
    pub fn with_backend_installed_now(mut self, installed: bool) -> Self {
        self.backend_installed_now = Some(installed);
        self
    }

    #[cfg(test)]
    pub fn with_cache_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
    }

    /// Binary this sandbox probes for. Anything other than `zerobox` uses the
    /// bwrap backend.
    fn backend_binary(&self) -> &'static str {
        if self.backend == "zerobox" {
            "zerobox"
        } else {
            "bwrap"
        }
    }

    fn backend_available(&self) -> bool {
        if let Some(available) = self.backend_available {
            return available;
        }
        if self.backend == "zerobox" {
            zerobox_exists()
        } else {
            bwrap_exists()
        }
    }

    /// Same probe without the process-wide cache, so a backend installed while
    /// the session is running is picked up instead of staying "missing".
    fn backend_available_now(&self) -> bool {
        if let Some(available) = self.backend_installed_now.or(self.backend_available) {
            return available;
        }
        which_cmd(self.backend_binary())
    }

    /// `Some(reason)` when the sandbox is required but its backend is missing,
    /// meaning bash commands must be refused rather than run unsandboxed.
    pub fn refusal_reason(&self) -> Option<String> {
        if !self.enabled || !self.required {
            return None;
        }
        if self.backend_available() || self.backend_available_now() {
            return None;
        }
        Some(format!(
            "sandbox backend '{}' not found; refusing to run because sandbox-required is set",
            self.backend
        ))
    }

    fn bare_command(&self, command: &str) -> Command {
        let mut cmd = Command::new(&self.shell);
        cmd.arg("-c").arg(command);
        configure_child_lifetime(&mut cmd);
        cmd
    }

    pub fn wrap_command(&self, command: &str) -> std::io::Result<Command> {
        if !self.enabled {
            return Ok(self.bare_command(command));
        }

        if let Some(reason) = self.refusal_reason() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, reason));
        }

        if !self.required && !self.backend_available() {
            tracing::warn!(
                "sandbox: {} not found, running unsandboxed",
                self.backend_binary()
            );
            return Ok(self.bare_command(command));
        }

        let cwd = std::env::current_dir().unwrap_or_default();

        if self.backend == "zerobox" {
            let mut cmd = Command::new("zerobox");
            cmd.arg("--allow-write");
            cmd.arg(cwd.as_os_str());
            cmd.arg("--");
            cmd.arg(&self.shell);
            cmd.arg("-c");
            cmd.arg(command);
            configure_child_lifetime(&mut cmd);
            return Ok(cmd);
        }

        let mut cmd = Command::new("bwrap");
        cmd.arg("--clearenv");
        for (k, v) in essential_env() {
            cmd.arg("--setenv").arg(k).arg(v);
        }
        match std::fs::canonicalize("/etc/resolv.conf") {
            Ok(target) => {
                cmd.arg("--ro-bind-try");
                cmd.arg(target);
                cmd.arg("/etc/resolv.conf");
            }
            Err(e) => {
                tracing::warn!(
                    "sandbox: no resolver file could be mounted: could not resolve /etc/resolv.conf: {}",
                    e
                );
            }
        }
        // must bind /etc/resolv.conf before /.
        cmd.args(["--ro-bind", "/", "/", "--bind"]);
        cmd.arg(cwd.as_os_str());
        cmd.arg(cwd.as_os_str());
        // Bind ~/.cache (or $XDG_CACHE_HOME) as writable after "/" bind
        if let Some(cache_dir) = self.cache_dir.clone().or_else(dirs::cache_dir) {
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                tracing::warn!(
                    "sandbox: failed to create cache dir {}: {e}",
                    cache_dir.display()
                );
            }
            cmd.arg("--bind");
            cmd.arg(cache_dir.as_os_str());
            cmd.arg(cache_dir.as_os_str());
        }
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
        ]);
        cmd.args([
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-cgroup",
            "--die-with-parent",
            &self.shell,
            "-c",
            command,
        ]);
        configure_child_lifetime(&mut cmd);
        Ok(cmd)
    }

    pub async fn output_command(&self, command: &str) -> std::io::Result<Output> {
        let mut cmd = self.wrap_command(command)?;
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let (stdout_handle, stdout) = spawn_pipe_reader(child.stdout.take());
        let (stderr_handle, stderr) = spawn_pipe_reader(child.stderr.take());
        let mut guard = ProcessGroupGuard::new(child.id(), self.active_groups.clone());
        let status = child.wait().await?;

        if tokio::time::timeout(std::time::Duration::from_millis(100), async {
            join_reader(stdout_handle).await?;
            join_reader(stderr_handle).await
        })
        .await
        .is_err()
            && let Some(pid) = guard.pid
        {
            kill_process_group(pid);
        }
        let stdout = stdout.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let stderr = stderr.lock().unwrap_or_else(|e| e.into_inner()).clone();
        guard.disarm();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    pub fn kill_active(&self) {
        let groups: Vec<u32> = self
            .active_groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .collect();
        for pid in groups {
            kill_process_group(pid);
        }
    }

    #[allow(dead_code)]
    pub fn active_group_count(&self) -> usize {
        self.active_groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

fn spawn_pipe_reader(
    pipe: Option<impl tokio::io::AsyncRead + Send + Unpin + 'static>,
) -> (
    tokio::task::JoinHandle<std::io::Result<()>>,
    Arc<Mutex<Vec<u8>>>,
) {
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_output = output.clone();
    let handle = tokio::spawn(async move {
        if let Some(mut pipe) = pipe {
            let mut buf = [0; 8192];
            loop {
                let read = pipe.read(&mut buf).await?;
                if read == 0 {
                    break;
                }
                reader_output
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&buf[..read]);
            }
        }
        Ok(())
    });
    (handle, output)
}

async fn join_reader(reader: tokio::task::JoinHandle<std::io::Result<()>>) -> std::io::Result<()> {
    reader
        .await
        .map_err(|e| std::io::Error::other(format!("pipe reader task failed: {e}")))?
}

pub(crate) fn configure_child_lifetime(cmd: &mut Command) {
    cmd.kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
}

pub(crate) fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        let group = format!("-{}", pid);
        let _ = std::process::Command::new("kill")
            .args(["-TERM", "--", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
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
